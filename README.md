# launch

<img src="img/launch.svg" align="right" width="150">

**Your everyday helpers and every program on the PATH in one window, found as you type.**

![Rust](https://img.shields.io/badge/language-Rust-orange) ![Unlicense](https://img.shields.io/badge/license-Unlicense-green) ![Platform](https://img.shields.io/badge/platform-Linux-blue) ![Stay Amazing](https://img.shields.io/badge/Stay-Amazing-important)

The top half is your own list: volume, brightness, Bluetooth, the apps you open every day. It spreads across the window in columns. Below a line sits a search field, and under it every program whose name holds what you type. Part of the [Fe₂O₃ Rust terminal suite](https://github.com/isene/fe2o3).

It replaces rofi and a menu script in one.

## Using it

Bind it to a key in your window manager:

```
bind F12 exec launch
```

From a key binding it opens its own [glass](https://github.com/isene/glass) window. The same key closes it again.

| Key | Does |
|---|---|
| type | Filter the helpers and the programs at once |
| arrows, `Tab` | Move; left and right jump between columns |
| `Enter` | Run the one under the cursor, or what you typed |
| `Ctrl-w` `Ctrl-u` | Delete a word, or all of it |
| `Esc` | Close |

Type `vol` and only Volume Up and Volume Down stay lit above. Type `gimp` and GIMP shows below.

## Your helpers

`~/.launch` holds them, one per line. The key in brackets is only shown, as a reminder:

```
Mute On [Ctrl+F1] = wpctl set-mute @DEFAULT_AUDIO_SINK@ 1
Volume Up [Ctrl+F3] = wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 10%+

Bluetooth On = rfkill unblock bluetooth
Bluetooth Off = rfkill block bluetooth
```

A blank line starts a new group. A group never splits across two columns.

## Programs

launch reads the list that the [bare](https://github.com/isene/bare) shell keeps in `~/.bare_exe_cache`, so it never walks the PATH itself. Without that file it looks through the PATH once.

## A password prompt

```bash
launch --password "Master password"
```

Opens a window, hides what you type, and prints it. The answer comes back through a pipe, so it never lands in a file. Nothing typed gives exit status 1.

## Speed

launch draws its first frame about 5 ms after it starts. Most of the wait is the terminal window opening. Between keys it does nothing at all.

## Install

```bash
cargo install --path .
```

## License

Public domain. Do what you like with it.
