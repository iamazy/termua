<p align="center">
  English | <a href="./README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <img src="./assets/logo/termua.svg" alt="Termua Logo" width="120" />
</p>

<p align="center">
  an open-source cross-platform terminal application built with <a href="https://github.com/zed-industries/zed">GPUI</a> and powered by the <a href="https://github.com/alacritty/alacritty">Alacritty</a> / <a href="https://github.com/wezterm/wezterm">WezTerm</a> terminal backends.
</p>

<p>
    <div align="center">
      <a href="https://github.com/iamazy/termua/releases">
        <img alt="Linux" src="https://img.shields.io/badge/-Linux-yellow?style=flat-square&logo=linux&logoColor=black&color=orange" />
      </a>
      <a href="https://github.com/iamazy/termua/releases">
        <img alt="Windows" src="https://img.shields.io/badge/-Windows-blue?style=flat-square&logo=windows&logoColor=white" />
      </a>
      <a href="https://github.com/iamazy/termua/releases">
        <img alt="macOS" src="https://img.shields.io/badge/-macOS-black?style=flat-square&logo=apple&logoColor=white" />
      </a>
    </div>
</p>

<div align="center">
    <img src="assets/screenshot/screenshot.png" alt="termua" height="500" style="border-radius: 16px;" />
</div>

### Features ❇️

- [x] Cross-platform: Linux / macOS / Windows
- [x] Terminal backends: supports Alacritty / WezTerm
- [x] SSH: based on `wezterm-ssh`, with support for Password / SSH Config login
- [x] Serial: supports serial sessions, baud rate / parity / flow control configuration
- [x] SFTP file operations: supports file upload (including drag-and-drop), concurrency control, and more
- [x] Terminal sharing: share terminal sessions through a relay
- [x] Cast recording and playback: record and replay terminal activity
- [x] Batch execution: run commands across multiple terminals
- [x] AI assistant: built-in ZeroClaw Assistant
- [x] Multiple themes: supports theme switching, plus creating and editing themes with the theme editor
- [x] Lock screen: application lock screen and automatic lock on idle timeout
- [x] Static suggestions: preconfigured static command suggestions with wildcard support

### Roadmap 🏁

- [ ] Support Lua scripting for more customizable scenarios
- [ ] Support workflows
- [ ] ...

### Quick Start

#### Play cast files

In addition to the built-in GUI recording and playback features, Termua can also play cast files directly from the command line:

```bash
termua --play-cast demo.cast
termua --play-cast demo.cast --speed 2
```

#### Terminal session sharing

In addition to starting a relay process with `termua-relay`, you can also start a local relay process from the Termua settings page for testing:

```bash
termua-relay --listen 127.0.0.1:7231
```

During a shared session, viewers can request control of the terminal, and the host side can revoke that control at any time.

### Configuration

#### Example `settings.json`

```json
{
  "appearance": {
    "theme": "system",
    "language": "zh-CN"
  },
  "terminal": {
    "default_backend": "alacritty",
    "ssh_backend": "ssh2",
    "font_family": ".ZedMono",
    "font_size": 15,
    "ligatures": true,
    "cursor_shape": "block",
    "blinking": "on",
    "option_as_meta": false,
    "show_scrollbar": true,
    "show_line_numbers": true,
    "copy_on_select": true,
    "suggestions_enabled": false,
    "suggestions_max_items": 8,
    "sftp_upload_max_concurrency": 5
  },
  "sharing": {
    "enabled": false,
    "relay_url": "ws://127.0.0.1:7231/ws"
  },
  "recording": {
    "include_input_by_default": false,
    "playback_speed": 1.0
  },
  "logging": {
    "level": "default",
    "path": "termua.log"
  }
}
```

It is not recommended to edit `settings.json` directly. Prefer changing settings through the `Termua` settings page.

### Release 🦀

You can download the binary from the [artifacts page](https://github.com/iamazy/termua/actions)

### Acknowledgements ❤️

- [GPUI](https://github.com/zed-industries/zed): GPUI is a hybrid immediate and retained mode, GPU accelerated UI framework for Rust, designed to support a wide variety of applications.
- [gpui-component](https://github.com/longbridge/gpui-component): Rust GUI components for building fantastic cross-platform desktop applications with GPUI.
- [Alacritty](https://github.com/alacritty/alacritty): A cross-platform, OpenGL terminal emulator.
- [WezTerm](https://github.com/wezterm/wezterm): A GPU-accelerated cross-platform terminal emulator and multiplexer implemented in Rust.

### License 🚨

<a href="./LICENSE-AGPL"><img src="https://img.shields.io/badge/license-AGPL%203-blue.svg" alt="License" /></a>
