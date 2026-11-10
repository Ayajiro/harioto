# harioto

*Harioto* (針音) is the faint hiss of a stylus tracking a record groove.

A terminal radio written in Rust. Seed it with one song and it keeps playing, following YouTube's mix recommendations. Around 16 MB resident, 3.4 MB binary.

## Why this one

Other terminal YouTube players each pay a price: YTerMusic and youtui use the internal YouTube Music API and need a browser cookie, which ties playback to your account; ytui-music, rs-pug and ytmusic-cli hand playback to an external `mpv` process.

harioto pays neither: **no login, no second process**. `yt-dlp` fetches the audio, `rodio` decodes it in-process. The trade is a smaller feature set — no account playlists, no artist/album browsing, just a radio that grows from a single song.

## Requirements

- `yt-dlp` — `brew install yt-dlp`
- Rust toolchain (built with 1.99.1)
- macOS or Linux

## Run

```sh
cargo run --release
```

| Command | Effect |
| --- | --- |
| `harioto` | Start the player |
| `harioto --selftest "query"` | Search, download, decode and play 5 seconds, printing to stdout |
| `harioto --prune 200` | Trim the audio cache to the given size in MB (default 500) |

`HARIOTO_CACHE_DIR` overrides the cache location.

## Keys

| Key | Action |
| --- | --- |
| `/` | Search YouTube |
| `r` | Start a radio from the playing track, or the selected one |
| `N` | New playlist |
| `a` | Add the selected track to the selected playlist |
| `d` | Remove the selected track from the open playlist |
| `Tab` | Switch focus between playlists and tracks |
| `j` `k` / arrows | Move the selection |
| `PageUp` `PageDown` `Home` `End` | Jump through the list |
| `Enter` | Open a playlist, or play a track |
| `Space` | Play / pause |
| `n` `b` | Next / previous |
| `←` `→` | Seek 5 seconds |
| `+` `-` | Volume |
| `s` | Stop |
| `q` / `Esc` | Quit |

## Radio

`r` takes the current track as a seed and loads the mix YouTube generates for it (`list=RD<videoID>`), up to 100 tracks in recommendation order. The seed keeps playing; the queue continues from it.

## Behaviour

- **Playback** — tracks download to the cache before playing, so seeking and replays are instant. The next track is prefetched while the current one plays.
- **Length check** — items with unknown length or longer than 20 minutes ask for a second `Enter` before playing, and are skipped during autoplay. This keeps livestreams and multi-hour compilations out of the queue.
- **Cache cap** — the audio cache stops at 500 MB, evicting least-recently-played files. Playing a file refreshes its timestamp; deleting a file that is currently playing is harmless.
- **yt-dlp freshness** — the version string is a release date. Past 60 days, the status bar suggests `brew upgrade yt-dlp`, since YouTube rotates its JavaScript challenge and a stale yt-dlp fails silently.

## Files

- Audio cache — `~/Library/Caches/harioto/audio/` (`~/.cache/harioto/audio/` on Linux)
- Playlists — `~/.config/harioto/playlists.json`
- Log — `~/Library/Caches/harioto/harioto.log`

## Limitations

- The first track waits for a full download, a few seconds; prefetching hides it afterwards.
- Decoding prefers m4a, as `symphonia` support for Opus is incomplete.

## License

MIT
