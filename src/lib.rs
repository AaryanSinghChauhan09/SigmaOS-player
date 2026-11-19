// SPDX-License-Identifier: MIT OR Apache-2.0
#![forbid(unsafe_code)]

//! # SigmaOS Player — mpv-backed Video Player
//!
//! Inspired by the Omarchy desktop philosophy, Hyprland's tiling paradigm,
//! and the Catppuccin Mocha colour palette — this crate models the full
//! lifecycle of media playback: file metadata, codec information, playback
//! state machine, subtitle track selection, speed/volume control, playlist
//! management, and a configurable keybinding map.
//!
//! All logic is pure-std; no external crates are required.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Every error that the player domain can emit.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerError {
    /// A required file could not be located.
    FileNotFound(PathBuf),
    /// The codec string was not recognised.
    UnknownCodec(String),
    /// A volume value outside 0–200 was supplied.
    VolumeOutOfRange(u8),
    /// A playback speed outside the allowed range was supplied.
    SpeedOutOfRange(f32),
    /// An operation was attempted while the player was in an incompatible state.
    InvalidState { current: PlaybackState, operation: &'static str },
    /// The requested subtitle track index does not exist.
    SubtitleTrackNotFound(usize),
    /// A keybinding action string was empty.
    EmptyAction,
    /// The playlist is empty when a track was expected.
    EmptyPlaylist,
    /// Duplicate keybinding detected.
    DuplicateKeybinding(String),
}

impl fmt::Display for PlayerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlayerError::FileNotFound(p) =>
                write!(f, "file not found: {}", p.display()),
            PlayerError::UnknownCodec(c) =>
                write!(f, "unknown codec: {c}"),
            PlayerError::VolumeOutOfRange(v) =>
                write!(f, "volume {v} is out of range 0–200"),
            PlayerError::SpeedOutOfRange(s) =>
                write!(f, "speed {s:.2}x is out of range 0.1–4.0"),
            PlayerError::InvalidState { current, operation } =>
                write!(f, "cannot perform '{operation}' while in state {current}"),
            PlayerError::SubtitleTrackNotFound(i) =>
                write!(f, "subtitle track {i} not found"),
            PlayerError::EmptyAction =>
                write!(f, "keybinding action must not be empty"),
            PlayerError::EmptyPlaylist =>
                write!(f, "playlist is empty"),
            PlayerError::DuplicateKeybinding(k) =>
                write!(f, "duplicate keybinding: {k}"),
        }
    }
}

impl std::error::Error for PlayerError {}

// ---------------------------------------------------------------------------
// Codec
// ---------------------------------------------------------------------------

/// Video or audio codec supported by the mpv backend.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Codec {
    H264,
    H265,
    Av1,
    Vp9,
    Vp8,
    Mpeg4,
    /// Audio-only codec.
    Aac,
    /// Audio-only codec.
    Mp3,
    /// Audio-only codec.
    Opus,
    /// Audio-only codec.
    Flac,
    /// Catch-all for less common codecs.
    Other(String),
}

impl fmt::Display for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Codec::H264     => write!(f, "H.264/AVC"),
            Codec::H265     => write!(f, "H.265/HEVC"),
            Codec::Av1      => write!(f, "AV1"),
            Codec::Vp9      => write!(f, "VP9"),
            Codec::Vp8      => write!(f, "VP8"),
            Codec::Mpeg4    => write!(f, "MPEG-4"),
            Codec::Aac      => write!(f, "AAC"),
            Codec::Mp3      => write!(f, "MP3"),
            Codec::Opus     => write!(f, "Opus"),
            Codec::Flac     => write!(f, "FLAC"),
            Codec::Other(s) => write!(f, "{s}"),
        }
    }
}

impl Codec {
    /// Parse a codec identifier string into a [`Codec`].
    pub fn parse(s: &str) -> Result<Self, PlayerError> {
        match s.to_ascii_lowercase().as_str() {
            "h264" | "avc"  => Ok(Codec::H264),
            "h265" | "hevc" => Ok(Codec::H265),
            "av1"           => Ok(Codec::Av1),
            "vp9"           => Ok(Codec::Vp9),
            "vp8"           => Ok(Codec::Vp8),
            "mpeg4" | "mp4v"=> Ok(Codec::Mpeg4),
            "aac"           => Ok(Codec::Aac),
            "mp3"           => Ok(Codec::Mp3),
            "opus"          => Ok(Codec::Opus),
            "flac"          => Ok(Codec::Flac),
            other if !other.is_empty() => Ok(Codec::Other(other.to_string())),
            _ => Err(PlayerError::UnknownCodec(s.to_string())),
        }
    }

    /// Whether this codec carries video frames.
    pub fn is_video(&self) -> bool {
        matches!(self, Codec::H264 | Codec::H265 | Codec::Av1
                      | Codec::Vp9 | Codec::Vp8 | Codec::Mpeg4)
    }
}

// ---------------------------------------------------------------------------
// Media file model
// ---------------------------------------------------------------------------

/// Metadata describing a media file on disk.
#[derive(Debug, Clone)]
pub struct MediaFile {
    pub path: PathBuf,
    pub video_codec: Option<Codec>,
    pub audio_codec: Option<Codec>,
    pub duration: Duration,
    /// Average bitrate in kilobits per second.
    pub bitrate_kbps: u32,
    /// Pixel dimensions, if the file contains video.
    pub resolution: Option<(u32, u32)>,
    pub title: Option<String>,
}

impl MediaFile {
    /// Begin constructing a [`MediaFile`] with its mandatory path.
    pub fn builder(path: impl Into<PathBuf>) -> MediaFileBuilder {
        MediaFileBuilder {
            path: path.into(),
            video_codec: None,
            audio_codec: None,
            duration: Duration::ZERO,
            bitrate_kbps: 0,
            resolution: None,
            title: None,
        }
    }

    /// Human-readable file name (lossy).
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Return `true` if this file contains a video stream.
    pub fn has_video(&self) -> bool {
        self.video_codec.as_ref().map_or(false, |c| c.is_video())
    }

    /// Format duration as `HH:MM:SS`.
    pub fn duration_display(&self) -> String {
        let total = self.duration.as_secs();
        let h = total / 3600;
        let m = (total % 3600) / 60;
        let s = total % 60;
        format!("{h:02}:{m:02}:{s:02}")
    }
}

impl fmt::Display for MediaFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} [{}]", self.file_name(), self.duration_display())
    }
}

/// Fluent builder for [`MediaFile`].
#[derive(Debug)]
pub struct MediaFileBuilder {
    path: PathBuf,
    video_codec: Option<Codec>,
    audio_codec: Option<Codec>,
    duration: Duration,
    bitrate_kbps: u32,
    resolution: Option<(u32, u32)>,
    title: Option<String>,
}

impl MediaFileBuilder {
    pub fn video_codec(mut self, codec: Codec) -> Self {
        self.video_codec = Some(codec);
        self
    }
    pub fn audio_codec(mut self, codec: Codec) -> Self {
        self.audio_codec = Some(codec);
        self
    }
    pub fn duration_secs(mut self, secs: u64) -> Self {
        self.duration = Duration::from_secs(secs);
        self
    }
    pub fn bitrate_kbps(mut self, kbps: u32) -> Self {
        self.bitrate_kbps = kbps;
        self
    }
    pub fn resolution(mut self, w: u32, h: u32) -> Self {
        self.resolution = Some((w, h));
        self
    }
    pub fn title(mut self, t: impl Into<String>) -> Self {
        self.title = Some(t.into());
        self
    }
    pub fn build(self) -> MediaFile {
        MediaFile {
            path: self.path,
            video_codec: self.video_codec,
            audio_codec: self.audio_codec,
            duration: self.duration,
            bitrate_kbps: self.bitrate_kbps,
            resolution: self.resolution,
            title: self.title,
        }
    }
}

// ---------------------------------------------------------------------------
// Subtitle track
// ---------------------------------------------------------------------------

/// Language tag following BCP 47 (e.g. `"en"`, `"pt-BR"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageTag(String);

impl LanguageTag {
    pub fn new(tag: impl Into<String>) -> Self {
        LanguageTag(tag.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LanguageTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A subtitle track embedded in or sideloaded alongside the media file.
#[derive(Debug, Clone)]
pub struct SubtitleTrack {
    pub index: usize,
    pub language: LanguageTag,
    pub label: String,
    pub is_forced: bool,
}

impl fmt::Display for SubtitleTrack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let forced = if self.is_forced { " [forced]" } else { "" };
        write!(f, "[{}] {} — {}{}", self.index, self.language, self.label, forced)
    }
}

// ---------------------------------------------------------------------------
// Playback state machine
// ---------------------------------------------------------------------------

/// Every state the player can be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Stopped,
    Playing,
    Paused,
    Buffering,
}

impl fmt::Display for PlaybackState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlaybackState::Stopped   => write!(f, "Stopped"),
            PlaybackState::Playing   => write!(f, "Playing"),
            PlaybackState::Paused    => write!(f, "Paused"),
            PlaybackState::Buffering => write!(f, "Buffering"),
        }
    }
}

// ---------------------------------------------------------------------------
// Keybinding map
// ---------------------------------------------------------------------------

/// A single keybinding: one keyboard shortcut → one named action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keybinding {
    pub key: String,
    pub action: String,
}

/// A validated map of keyboard shortcuts to player actions.
#[derive(Debug, Default)]
pub struct KeybindingMap {
    bindings: HashMap<String, String>,
}

impl KeybindingMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new keybinding. Returns an error if the action is empty or
    /// the key is already bound.
    pub fn bind(&mut self, key: impl Into<String>, action: impl Into<String>)
        -> Result<(), PlayerError>
    {
        let key = key.into();
        let action = action.into();
        if action.trim().is_empty() {
            return Err(PlayerError::EmptyAction);
        }
        if self.bindings.contains_key(&key) {
            return Err(PlayerError::DuplicateKeybinding(key));
        }
        self.bindings.insert(key, action);
        Ok(())
    }

    /// Look up the action for a key.
    pub fn action_for(&self, key: &str) -> Option<&str> {
        self.bindings.get(key).map(|s| s.as_str())
    }

    /// Remove a keybinding, returning the old action if it existed.
    pub fn unbind(&mut self, key: &str) -> Option<String> {
        self.bindings.remove(key)
    }

    /// Iterate over all (key, action) pairs in an unspecified order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.bindings.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Total number of registered bindings.
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// Return the default Omarchy-inspired keybinding set.
    pub fn default_bindings() -> Self {
        let mut map = Self::new();
        for (k, a) in [
            ("space",    "toggle-pause"),
            ("q",        "quit"),
            ("f",        "toggle-fullscreen"),
            ("m",        "toggle-mute"),
            ("right",    "seek-forward-5"),
            ("left",     "seek-backward-5"),
            ("up",       "volume-up-5"),
            ("down",     "volume-down-5"),
            (".",        "frame-step"),
            (",",        "frame-step-back"),
            ("l",        "playlist-next"),
            ("h",        "playlist-prev"),
            ("s",        "cycle-subtitles"),
        ] {
            map.bind(k, a).expect("default bindings are unique and non-empty");
        }
        map
    }
}

// ---------------------------------------------------------------------------
// Playlist
// ---------------------------------------------------------------------------

/// An ordered list of media files with a cursor tracking the current item.
#[derive(Debug, Default)]
pub struct Playlist {
    items: Vec<MediaFile>,
    cursor: usize,
    /// Whether to cycle back to the start after the last item.
    pub loop_playlist: bool,
}

impl Playlist {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, file: MediaFile) {
        self.items.push(file);
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The currently selected [`MediaFile`], if any.
    pub fn current(&self) -> Option<&MediaFile> {
        self.items.get(self.cursor)
    }

    /// Advance to the next item. Returns `Err` if the playlist is empty or the
    /// end has been reached and looping is disabled.
    pub fn next(&mut self) -> Result<&MediaFile, PlayerError> {
        if self.items.is_empty() {
            return Err(PlayerError::EmptyPlaylist);
        }
        if self.cursor + 1 < self.items.len() {
            self.cursor += 1;
        } else if self.loop_playlist {
            self.cursor = 0;
        } else {
            return Err(PlayerError::EmptyPlaylist);
        }
        Ok(&self.items[self.cursor])
    }

    /// Step back to the previous item.
    pub fn prev(&mut self) -> Result<&MediaFile, PlayerError> {
        if self.items.is_empty() {
            return Err(PlayerError::EmptyPlaylist);
        }
        if self.cursor > 0 {
            self.cursor -= 1;
        } else if self.loop_playlist {
            self.cursor = self.items.len() - 1;
        } else {
            return Err(PlayerError::EmptyPlaylist);
        }
        Ok(&self.items[self.cursor])
    }

    /// Jump directly to position `idx`.
    pub fn jump(&mut self, idx: usize) -> Result<&MediaFile, PlayerError> {
        if idx >= self.items.len() {
            return Err(PlayerError::EmptyPlaylist);
        }
        self.cursor = idx;
        Ok(&self.items[self.cursor])
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }
}

// ---------------------------------------------------------------------------
// Player controller
// ---------------------------------------------------------------------------

/// Top-level player controller.
///
/// Owns the playlist, tracks playback state, and enforces invariants
/// (e.g. volume range, speed range) so the mpv subprocess is never handed
/// invalid values.
#[derive(Debug)]
pub struct Player {
    state: PlaybackState,
    /// Volume: 0–200 (100 = original level).
    volume: u8,
    /// Playback speed multiplier: 0.1–4.0.
    speed: f32,
    muted: bool,
    pub playlist: Playlist,
    subtitle_tracks: Vec<SubtitleTrack>,
    active_subtitle: Option<usize>,
    pub keybindings: KeybindingMap,
}

impl Player {
    /// Create a new player with sane defaults and the standard keybinding map.
    pub fn new() -> Self {
        Player {
            state: PlaybackState::Stopped,
            volume: 100,
            speed: 1.0,
            muted: false,
            playlist: Playlist::new(),
            subtitle_tracks: Vec::new(),
            active_subtitle: None,
            keybindings: KeybindingMap::default_bindings(),
        }
    }

    // --- state machine ---

    pub fn state(&self) -> PlaybackState { self.state }

    /// Start or resume playback. Requires at least one file in the playlist.
    pub fn play(&mut self) -> Result<(), PlayerError> {
        if self.playlist.is_empty() {
            return Err(PlayerError::EmptyPlaylist);
        }
        match self.state {
            PlaybackState::Stopped
            | PlaybackState::Paused
            | PlaybackState::Buffering => {
                self.state = PlaybackState::Playing;
                Ok(())
            }
            PlaybackState::Playing => Ok(()), // idempotent
        }
    }

    /// Pause playback.
    pub fn pause(&mut self) -> Result<(), PlayerError> {
        match self.state {
            PlaybackState::Playing | PlaybackState::Buffering => {
                self.state = PlaybackState::Paused;
                Ok(())
            }
            other => Err(PlayerError::InvalidState {
                current: other,
                operation: "pause",
            }),
        }
    }

    /// Stop playback and reset position.
    pub fn stop(&mut self) {
        self.state = PlaybackState::Stopped;
    }

    /// Enter buffering state (called by the mpv IPC layer when network stalls).
    pub fn set_buffering(&mut self) -> Result<(), PlayerError> {
        match self.state {
            PlaybackState::Playing => {
                self.state = PlaybackState::Buffering;
                Ok(())
            }
            other => Err(PlayerError::InvalidState {
                current: other,
                operation: "buffer",
            }),
        }
    }

    // --- volume / speed ---

    pub fn volume(&self) -> u8 { self.volume }

    /// Set volume in the range 0–200.
    pub fn set_volume(&mut self, v: u8) -> Result<(), PlayerError> {
        if v > 200 {
            return Err(PlayerError::VolumeOutOfRange(v));
        }
        self.volume = v;
        Ok(())
    }

    pub fn muted(&self) -> bool { self.muted }

    pub fn toggle_mute(&mut self) { self.muted = !self.muted; }

    pub fn speed(&self) -> f32 { self.speed }

    /// Set playback speed in the range 0.1–4.0.
    pub fn set_speed(&mut self, s: f32) -> Result<(), PlayerError> {
        if !(0.1..=4.0).contains(&s) {
            return Err(PlayerError::SpeedOutOfRange(s));
        }
        self.speed = s;
        Ok(())
    }

    // --- subtitles ---

    /// Register a subtitle track discovered from the loaded file.
    pub fn add_subtitle_track(&mut self, track: SubtitleTrack) {
        self.subtitle_tracks.push(track);
    }

    /// All available subtitle tracks.
    pub fn subtitle_tracks(&self) -> &[SubtitleTrack] {
        &self.subtitle_tracks
    }

    /// Select the subtitle track at `index` (the [`SubtitleTrack::index`] field).
    pub fn select_subtitle(&mut self, index: usize) -> Result<(), PlayerError> {
        if self.subtitle_tracks.iter().any(|t| t.index == index) {
            self.active_subtitle = Some(index);
            Ok(())
        } else {
            Err(PlayerError::SubtitleTrackNotFound(index))
        }
    }

    /// Disable subtitles.
    pub fn disable_subtitles(&mut self) {
        self.active_subtitle = None;
    }

    pub fn active_subtitle(&self) -> Option<usize> {
        self.active_subtitle
    }
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Codec ---

    #[test]
    fn test_codec_parse_known_variants() {
        // Arrange
        let cases = [
            ("h264", Codec::H264),
            ("hevc", Codec::H265),
            ("av1",  Codec::Av1),
            ("vp9",  Codec::Vp9),
            ("aac",  Codec::Aac),
            ("opus", Codec::Opus),
        ];
        for (input, expected) in cases {
            // Act
            let result = Codec::parse(input).unwrap();
            // Assert
            assert_eq!(result, expected, "codec '{input}' did not parse correctly");
        }
    }

    #[test]
    fn test_codec_is_video() {
        // Arrange / Act / Assert
        assert!(Codec::H264.is_video());
        assert!(Codec::Av1.is_video());
        assert!(!Codec::Aac.is_video());
        assert!(!Codec::Flac.is_video());
    }

    // --- MediaFile builder ---

    #[test]
    fn test_media_file_builder_and_display() {
        // Arrange
        let file = MediaFile::builder("/videos/demo.mkv")
            .video_codec(Codec::H265)
            .audio_codec(Codec::Opus)
            .duration_secs(7384) // 2h 3m 4s
            .bitrate_kbps(4000)
            .resolution(1920, 1080)
            .title("Demo Movie")
            .build();

        // Act
        let display = file.to_string();
        let dur = file.duration_display();

        // Assert
        assert!(display.contains("demo.mkv"));
        assert_eq!(dur, "02:03:04");
        assert!(file.has_video());
        assert_eq!(file.bitrate_kbps, 4000);
    }

    #[test]
    fn test_media_file_no_video_stream() {
        // Arrange
        let file = MediaFile::builder("/music/track.flac")
            .audio_codec(Codec::Flac)
            .duration_secs(240)
            .build();
        // Act + Assert
        assert!(!file.has_video());
        assert!(file.resolution.is_none());
    }

    // --- Playback state machine ---

    #[test]
    fn test_player_play_pause_stop_cycle() {
        // Arrange
        let mut player = Player::new();
        let media = MediaFile::builder("/tmp/x.mp4")
            .video_codec(Codec::H264)
            .duration_secs(60)
            .build();
        player.playlist.push(media);

        // Act & Assert — play
        player.play().expect("should play");
        assert_eq!(player.state(), PlaybackState::Playing);

        // Act & Assert — pause
        player.pause().expect("should pause");
        assert_eq!(player.state(), PlaybackState::Paused);

        // Act & Assert — stop
        player.stop();
        assert_eq!(player.state(), PlaybackState::Stopped);
    }

    #[test]
    fn test_player_pause_from_stopped_returns_error() {
        // Arrange
        let mut player = Player::new();
        // Act
        let result = player.pause();
        // Assert
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(),
            PlayerError::InvalidState { current: PlaybackState::Stopped, .. }));
    }

    #[test]
    fn test_player_play_empty_playlist_returns_error() {
        // Arrange
        let mut player = Player::new();
        // Act
        let result = player.play();
        // Assert
        assert_eq!(result.unwrap_err(), PlayerError::EmptyPlaylist);
    }

    // --- Volume & speed ---

    #[test]
    fn test_volume_valid_and_invalid() {
        // Arrange
        let mut player = Player::new();

        // Act / Assert — valid
        player.set_volume(150).expect("150 is valid");
        assert_eq!(player.volume(), 150);

        // Act / Assert — boundary
        player.set_volume(200).expect("200 is valid boundary");
        assert_eq!(player.volume(), 200);

        // Act / Assert — invalid
        let err = player.set_volume(201).unwrap_err();
        assert_eq!(err, PlayerError::VolumeOutOfRange(201));
    }

    #[test]
    fn test_speed_valid_and_invalid() {
        // Arrange
        let mut player = Player::new();

        // Act / Assert — valid
        player.set_speed(2.0).expect("2.0 is valid");
        assert!((player.speed() - 2.0).abs() < f32::EPSILON);

        // Act / Assert — invalid low
        let err = player.set_speed(0.0).unwrap_err();
        assert!(matches!(err, PlayerError::SpeedOutOfRange(_)));

        // Act / Assert — invalid high
        let err2 = player.set_speed(5.0).unwrap_err();
        assert!(matches!(err2, PlayerError::SpeedOutOfRange(_)));
    }

    // --- Subtitle tracks ---

    #[test]
    fn test_subtitle_track_selection() {
        // Arrange
        let mut player = Player::new();
        player.add_subtitle_track(SubtitleTrack {
            index: 0,
            language: LanguageTag::new("en"),
            label: "English".into(),
            is_forced: false,
        });
        player.add_subtitle_track(SubtitleTrack {
            index: 1,
            language: LanguageTag::new("pt-BR"),
            label: "Portuguese (Brazil)".into(),
            is_forced: true,
        });

        // Act
        player.select_subtitle(1).expect("track 1 exists");

        // Assert
        assert_eq!(player.active_subtitle(), Some(1));

        // Act — disable
        player.disable_subtitles();
        assert_eq!(player.active_subtitle(), None);

        // Act — non-existent track
        let err = player.select_subtitle(99).unwrap_err();
        assert_eq!(err, PlayerError::SubtitleTrackNotFound(99));
    }

    // --- Keybinding map ---

    #[test]
    fn test_keybinding_default_and_lookup() {
        // Arrange
        let map = KeybindingMap::default_bindings();

        // Act + Assert — space → toggle-pause
        assert_eq!(map.action_for("space"), Some("toggle-pause"));
        assert_eq!(map.action_for("q"), Some("quit"));
        assert!(map.action_for("nonexistent").is_none());
    }

    #[test]
    fn test_keybinding_duplicate_and_empty_action() {
        // Arrange
        let mut map = KeybindingMap::new();
        map.bind("ctrl+l", "playlist-next").unwrap();

        // Act — duplicate key
        let dup_err = map.bind("ctrl+l", "some-action").unwrap_err();
        assert_eq!(dup_err, PlayerError::DuplicateKeybinding("ctrl+l".into()));

        // Act — empty action
        let empty_err = map.bind("ctrl+k", "   ").unwrap_err();
        assert_eq!(empty_err, PlayerError::EmptyAction);
    }

    // --- Playlist ---

    #[test]
    fn test_playlist_navigation_and_looping() {
        // Arrange
        let mut pl = Playlist::new();
        pl.loop_playlist = true;
        for name in ["a.mp4", "b.mp4", "c.mp4"] {
            pl.push(MediaFile::builder(format!("/tmp/{name}")).duration_secs(60).build());
        }

        // Act — advance past end → wraps
        pl.next().unwrap(); // cursor = 1
        pl.next().unwrap(); // cursor = 2
        pl.next().unwrap(); // cursor = 0 (looped)

        // Assert
        assert_eq!(pl.cursor(), 0);
        assert_eq!(pl.current().unwrap().file_name(), "a.mp4");

        // Act — prev wraps from 0 → 2
        pl.prev().unwrap();
        assert_eq!(pl.cursor(), 2);
    }

    #[test]
    fn test_playlist_next_no_loop_at_end_returns_error() {
        // Arrange
        let mut pl = Playlist::new();
        pl.loop_playlist = false;
        pl.push(MediaFile::builder("/tmp/only.mp4").duration_secs(10).build());

        // Act — already at end, no loop
        let err = pl.next().unwrap_err();
        assert_eq!(err, PlayerError::EmptyPlaylist);
    }

    // --- Toggle mute ---

    #[test]
    fn test_mute_toggle() {
        // Arrange
        let mut player = Player::new();
        assert!(!player.muted());

        // Act
        player.toggle_mute();
        // Assert
        assert!(player.muted());

        // Act
        player.toggle_mute();
        // Assert
        assert!(!player.muted());
    }
}
