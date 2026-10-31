# SigmaOS Player

Python CLI that plans mpv playback arguments and parses playlists, metadata,
and seek requests. It prints a shell-escaped command plan for the caller.

## Playback boundary

The current CLI does not execute mpv or provide a graphical player. A printed
plan does not establish that mpv is installed, that a media URL is reachable,
or that playback will succeed. Keep process execution as a separate layer that
passes an argument vector directly rather than evaluating the printed string
through a shell.

## Development

`make test` runs deterministic planning and parsing checks without spawning
mpv or accessing the network. The project studies [Xplayer](https://github.com/linuxmint/xplayer)
and [Opal](https://github.com/debpalash/Opal) as references; it does not claim
feature parity with either player.
