# TypeSuggest for Omarchy

An [Omarchy](https://omarchy.org) 4 (Quattro) shell plugin for
[TypeSuggest](https://github.com/AbdulrahmanHR/typesuggest), the Wayland input
method that shows Windows-style word suggestions at the text cursor while you
type.

It adds a keyboard icon to the Omarchy bar. The icon is dimmed while
TypeSuggest is off, and its tooltip says whether it is on. Clicking it opens a
panel styled like Omarchy's own panels:

- An on/off switch. It starts or stops the TypeSuggest service and turns
  autostart on or off to match, so the setting survives a log out.
- Quick settings: where the suggestion bar sits (below or above the cursor),
  bar size (0.75x to 2x), how many suggestions to show (1 to 5), which keys
  accept a suggestion (Enter, Space, Tab; at least one stays on), learning,
  typo correction, and colors (follow the Omarchy theme, or TypeSuggest's
  default palette).
- **Clear learned phrases**, after a confirmation.
- **Open config file**, which opens `~/.config/typesuggest/config.toml` in your
  editor the way Omarchy opens its own config files.

Changes apply the next time a text field is focused; there is no restart.

If TypeSuggest is not installed, the panel explains what it is and offers an
**Install TypeSuggest** button (see [below](#what-the-install-button-does)).

## Requirements

- Omarchy 4 (Quattro), with the default `omarchy.bar` (or another bar that
  hosts bar widgets).
- TypeSuggest, from the AUR package
  [`typesuggest`](https://aur.archlinux.org/packages/typesuggest). The quick
  settings need a build whose `typesuggest --help` lists `--config-json` and
  `--set`; with an older build the panel still has the on/off switch, but asks
  you to update before it shows the settings. The panel can install
  TypeSuggest for you.

### External dependencies

The plugin itself is QML only. It runs these programs, all of which Omarchy
already ships except TypeSuggest itself:

| Program | Used for |
|---|---|
| `typesuggest` (AUR package `typesuggest`) | Reading and changing settings, autostart, clearing learned phrases |
| `systemctl` (systemd user session) | Service state, start and stop |
| `sh` | The installed check (`command -v typesuggest`) |
| `omarchy-launch-config-editor` | **Open config file** |
| `omarchy-launch-floating-terminal-with-presentation`, `bash` | **Install TypeSuggest** |
| `omarchy-pkg-aur-add` (uses `yay`), `gum` | Inside the install script |

## Install

```bash
omarchy plugin add https://github.com/AbdulrahmanHR/omarchy-typesuggest
```

Omarchy asks before cloning, then offers to enable the plugin and place it on
the bar (it goes to the right side by default). Plugins run as unsandboxed code
inside the Omarchy shell, so read the code before you enable it. To enable it
later:

```bash
omarchy plugin enable io.github.abdulrahmanhr.typesuggest
```

## Using it

- **Left click** the icon: open the panel.
- **Right click** the icon: turn TypeSuggest on or off.

In the panel, as in Omarchy's other panels: `j`/`k` or the arrow keys move
between rows, `h`/`l` move within a row (and step the bar size),
`Enter`/`Space` activate, `t` turns TypeSuggest on or off, `r` reloads the
state, `Tab` switches to the next bar panel, and `Esc` closes it.

The panel reads the state each time it opens. While the icon is on the bar it
also checks whether the service is running every 30 seconds, so the icon
follows changes made elsewhere (for example `typesuggest --toggle` on a
keybinding). The interval is the widget's `refreshIntervalSec` setting (5 to
3600 seconds).

## What the plugin runs

Every command is started as an argument list, never through a shell that
re-reads values, and every value comes from a fixed set of choices in the
panel.

| Action | Command |
|---|---|
| Is it installed? | `sh -c 'command -v typesuggest'` |
| Does this build support the settings? | `typesuggest --help` |
| Is it on? | `systemctl --user is-active typesuggest` |
| Read the settings | `typesuggest --config-json` |
| Change a setting | `typesuggest --set <key> <value>` with `bar_position`, `bar_scale`, `max_candidates`, `accept_keys`, `learn`, `typo_correction` or `theme` |
| Turn on | `typesuggest --enable-autostart`, then `systemctl --user start typesuggest` |
| Turn off | `systemctl --user stop typesuggest`, then `typesuggest --disable-autostart` |
| Clear learned phrases | `typesuggest --clear-learned` (TypeSuggest restarts its service so the daemon forgets them too) |
| Open config file | `omarchy-launch-config-editor ~/.config/typesuggest/config.toml` |
| Install TypeSuggest | `omarchy-launch-floating-terminal-with-presentation "bash <plugin dir>/scripts/install-typesuggest.sh"` |

`typesuggest --config-json` is only run after `--help` has shown that the
installed build knows it; older builds ignore unknown options and would start
a second copy of the daemon instead.

## What the Install button does

It opens a floating Omarchy terminal running
[`scripts/install-typesuggest.sh`](scripts/install-typesuggest.sh). The script
first prints every step and how to undo it, and changes nothing unless you
confirm. Then it:

1. Installs the AUR package: `omarchy-pkg-aur-add typesuggest`. This builds
   the package with `yay` and asks for your password to install it.
2. Turns off Fcitx5: `systemctl --user disable --now omarchy-fcitx5.service`.
   Only one input method can run at a time, and Omarchy starts Fcitx5 by
   default. Fcitx5 also provides Omarchy's CapsLock compose sequences
   (`~/.XCompose`), which stop working while it is off.
3. Starts TypeSuggest and enables it on login:
   `typesuggest --enable-autostart`, then `systemctl --user start typesuggest`.

It stops at the first failed step. It never downloads and runs anything
itself, and it does not touch your Omarchy or shell configuration.

Some apps need one extra setting before suggestions appear (Chromium and
Electron apps need `--enable-wayland-ime`; Qt apps on Omarchy need
`QT_IM_MODULE=wayland`). See
[App compatibility](https://github.com/AbdulrahmanHR/typesuggest#-app-compatibility)
in the TypeSuggest README.

## Remove

Remove the plugin:

```bash
omarchy plugin remove io.github.abdulrahmanhr.typesuggest
```

Remove TypeSuggest itself and bring Fcitx5 back:

```bash
systemctl --user stop typesuggest
typesuggest --disable-autostart
omarchy-pkg-drop typesuggest          # or: yay -R typesuggest
systemctl --user enable --now omarchy-fcitx5.service
rm -rf ~/.config/typesuggest          # optional: settings and learned phrases
```

## Privacy

The plugin never sees what you type. It only runs the commands listed above,
reads the settings TypeSuggest prints, and makes no network connections.

## License

[MIT](LICENSE)
