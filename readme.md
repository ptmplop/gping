# gping 🚀

[![Crates.io](https://img.shields.io/crates/v/gping.svg)](https://crates.io/crates/gping)
[![Actions Status](https://github.com/orf/gping/workflows/CI/badge.svg)](https://github.com/orf/gping/actions)

Ping, but with a graph.

![](./images/readme-example.gif)

Comes with the following super-powers:
* Graph the ping time for multiple hosts, each in a lane of its own
* Graph the _execution time_ for commands via the `--cmd` flag
* See at a glance when a host is slower than it usually is: its line warms through amber, orange and red
* Lost pings are hard to miss: a red curtain through the lane, a ✕ on the rail beneath it, and a caption on longer outages
* A live readout for every host, with stats for the window and the whole session
* Pause, rescale, hide hosts, and keep a session report in your scrollback when you quit
* Custom colours, 24-bit colour with fallbacks for 256- and 16-colour terminals
* Windows, Mac and Linux support

Table of Contents
=================

   * [Install :cd:](#install-cd)
   * [Usage :saxophone:](#usage-saxophone)

<a href="https://repology.org/project/gping/versions">
    <img src="https://repology.org/badge/vertical-allrepos/gping.svg" alt="Packaging status" align="right">
</a>

# Install :cd:

* macOS
  * [Homebrew](https://formulae.brew.sh/formula/gping#default): `brew install gping`
  * [MacPorts](https://ports.macports.org/port/gping/): `sudo port install gping`
* Linux (Homebrew): `brew install gping`
* CentOS (and other distributions with an old glibc): Download the MUSL build from the latest release
* Windows/ARM:
  * Scoop: `scoop install gping`
  * Chocolatey: `choco install gping`
  * WinGet: `winget.exe install orf.gping`
  * Download the latest release from [the github releases page](https://github.com/orf/gping/releases)
* Fedora ([COPR](https://copr.fedorainfracloud.org/coprs/atim/gping/)): `sudo dnf copr enable atim/gping -y && sudo dnf install gping`
* Cargo (**This requires `rustc` version 1.88.0 or greater**): `cargo install gping`
* From a checkout of this repository: `cargo install --locked --path gping`
* Arch Linux: `pacman -S gping`
* Alpine linux: `apk add gping`
* Ubuntu >23.10/Debian >13: `apt install gping`
* Ubuntu/Debian ([Azlux's repo](https://packages.azlux.fr/)):
  ```bash
  echo 'deb [signed-by=/usr/share/keyrings/azlux.gpg] https://packages.azlux.fr/debian/ bookworm main' | sudo tee /etc/apt/sources.list.d/azlux.list
  sudo apt install gpg curl
  curl -s https://azlux.fr/repo.gpg.key | gpg --dearmor | sudo tee /usr/share/keyrings/azlux.gpg > /dev/null
  sudo apt update
  sudo apt install gping
  ```
* Gentoo ([dm9pZCAq overlay](https://github.com/gentoo-mirror/dm9pZCAq)):
  ```sh
  sudo eselect repository enable dm9pZCAq
  sudo emerge --sync dm9pZCAq
  sudo emerge net-misc/gping::dm9pZCAq
  ```
* FreeBSD:
  * [pkg](https://www.freshports.org/net-mgmt/gping/): `pkg install gping`
  * [ports](https://cgit.freebsd.org/ports/tree/net-mgmt/gping) `cd /usr/ports/net-mgmt/gping; make install clean`
* Docker:
  ```sh
  # Check all options
  docker run --rm -ti --network host ghcr.io/orf/gping:gping-v1.15.1 --help
  # Ping google.com
  docker run --rm -ti --network host ghcr.io/orf/gping:gping-v1.15.1 google.com
  ```
* Flox:
  ```sh
  # Inside of a Flox environment
  flox install gping
  ```
* [gah](https://github.com/marverix/gah):
  ```sh
  gah install gping
  ```

# Usage :saxophone:

Just run `gping [host]`. `host` can be a command like `curl google.com` if the `--cmd` flag is used. You can also use
shorthands like `aws:eu-west-1`, `aws:ca-central-1` or `gcp:me-central2` to ping specific cloud regions.

## Reading the screen

![](./images/screen.png)

* Each host gets a lane. Its line scrolls from right to left, the newest reply at the right edge, on a scale
  fitted to that host (press `s` to put every lane on one scale).
* The line keeps its host's colour while the host answers as fast as it usually does, and turns amber, orange and
  then red where it's slower than its usual: the median of its last ten minutes, so a 7 ms resolver and a 180 ms
  path across an ocean are each judged against themselves.
* A lost ping breaks the line, drops a red curtain through the lane and puts a ✕ on the rail under it. Outages of a
  second or more are captioned with how long they lasted.
* The card on the right shows the latest latency in big digits, the host's status (steady, slow, lossy, down or
  waiting), its stats over the window, and a sparkline of the whole session. The bars beside it show where its
  latency usually sits.
* The title bar sums up how every host is doing.
* When you quit, the last frame is redrawn as a report of the whole session and stays in your scrollback (unless
  you pass `--clear`).

## Keys

| Key | |
|---|---|
| `space` or `p` | Pause the view; pinging carries on |
| `s` | One scale per host, or one shared scale |
| `1`-`9`, `0` | Hide or show a host; `0` shows them all |
| `r` | Restart the session totals |
| `?` | Help |
| `q`, `Esc` or `Ctrl-C` | Quit |

## Colour

gping draws in 24-bit colour on a background of its own, so it looks the same on any theme. It works out what your
terminal supports from `COLORTERM` and `TERM`, and falls back to 256 or 16 colours (with 16, lost pings are marked by
their ✕ alone, without the red curtain). You can override that:

* `GPING_COLOR=truecolor`, `256`, `16` or `none` picks the colour depth.
* `NO_COLOR=1` turns colour off; shapes, gaps and words still carry every meaning.
* `GPING_BG=none` keeps your terminal's own background, for transparent or tinted windows.

## Options

```bash
$ gping --help
Ping, but with a graph.

Usage: gping [OPTIONS] [HOSTS_OR_COMMANDS]...

Arguments:
  [HOSTS_OR_COMMANDS]...  Hosts or IPs to ping, or commands to run if --cmd is provided. Can use cloud shorthands like aws:eu-west-1

Options:
      --cmd
          Graph the execution time for a list of commands rather than pinging hosts
  -n, --watch-interval <WATCH_INTERVAL>
          Watch interval seconds (provide partial seconds like '0.5'). Default for ping is 0.2 (1.0 on the BSDs), default for cmd is 0.5
  -b, --buffer <BUFFER>
          Determines the number of seconds to display in the graph [default: 30]
  -4
          Resolve ping targets to IPv4 address
  -6
          Resolve ping targets to IPv6 address
  -i, --interface <INTERFACE>
          Interface to use when pinging
  -s, --simple-graphics
          Uses dot characters instead of braille
      --vertical-margin <VERTICAL_MARGIN>
          Vertical margin around the graph (top and bottom) [default: 1]
      --horizontal-margin <HORIZONTAL_MARGIN>
          Horizontal margin around the graph (left and right) [default: 0]
      --ymin <YMIN>
          Set vertical axis min (ms)
      --ymax <YMAX>
          Set vertical axis max (ms)
  -0
          Equivalent to --ymin=0
      --tcp
          Use TCP pings instead of ICMP
      --tcp-rst <TCP_RST>
          How to treat a connection refused (RST) when TCP pinging [default: pong] [possible values: pong, drop]
      --tcp-port <TCP_PORT>
          Port to connect to (only used for TCP pings) [default: 80]
  -c, --color <color>
          Assign color to a graph entry.

          This option can be defined more than once as a comma separated string, and the
          order which the colors are provided will be matched against the hosts or
          commands passed to gping.

          Hexadecimal RGB color codes are accepted in the form of '#RRGGBB' or the
          following color names: 'black', 'red', 'green', 'yellow', 'blue', 'magenta',
          'cyan', 'gray', 'dark-gray', 'light-red', 'light-green', 'light-yellow',
          'light-blue', 'light-magenta', 'light-cyan', and 'white'
      --clear
          Clear the graph from the terminal after closing the program
      --ping-args [<PING_ARGS>...]
          Extra arguments to pass to `ping`. These are platform dependent. Everything after --ping-args is passed on, so it has to come after the hosts
  -h, --help
          Print help (see more with '--help')
  -V, --version
          Print version
```
