> [!IMPORTANT]
> Remove this line to confirm you've reviewed this PR before submitting.

# Zed Unity

This fork includes UnityEvent reference navigation and keeps the Agent panel for external ACP agents, including Codex and Claude Code. Zed account sign-in and Zed’s native agent are disabled. Build the macOS app with `script/bundle-mac` using Zed's macOS build prerequisites, the .NET 8 SDK, and the Rust `wasm32-wasip2` target. Open **Zed Unity** and select **Install Unity support** during onboarding. You can return to setup through **zed: open onboarding**. The C# extension downloads from Zed's public registry. UnityEvent References and its runtime come from the app bundle.

Press **Shift** twice to open **Search Everywhere**. Start typing to find files, symbols, editor actions, and text in your project. Use **Tab** or **Shift+Tab** to switch categories, **Enter** to open a result, and **Esc** to return. Alternate keymaps are available under **Base Keymap** in Settings.

Middle-click a symbol to open a quick usages modal with file paths, line numbers, and highlighted code snippets. Filter the list, press Enter to open a usage, or choose **Open in tab** to show all references in the full dedicated tab view.

Use the selector above the project tree to switch views. **Unity** shows `Assets`, `Packages`, and `ProjectSettings` without `.meta` files. **Solution** filters the tree to .NET source, solution, project, and assembly definition files, excluding generated caches. **Files** shows the regular file tree. Click the crosshair button, **Select Opened File**, to expand and highlight the active file. It switches to Files when the selected view excludes that file. The same command is available in the command palette as **project panel: select opened file**.

Place the caret on a C# class or method declaration to see its usage count after inline Git blame. Click the count to open the references. When a scene or prefab is among the results, click **Unity objects** to see the GameObject names, asset paths, and line numbers. For a component class, the list shows its attachments. For a method, it shows the objects that own its serialized UnityEvent listeners. The same names appear when hovering over the declaration. Unity asset lookup covers text-serialized scenes and prefabs under `Assets`, including direct component attachments and persistent calls to local component targets. Inherited callbacks, overloaded UnityEvent methods, and prefab instance overrides are outside its scope.

For a development build, place the archive beside the editor binary before launch, for example `script/package-unity-extension osx-arm64 target/debug/unity-event-references.tar.gz`. Use the .NET runtime identifier matching the editor's target architecture.

# Zed

[![Zed](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/zed-industries/zed/main/assets/badge/v0.json)](https://zed.dev)
[![CI](https://github.com/zed-industries/zed/actions/workflows/run_tests.yml/badge.svg)](https://github.com/zed-industries/zed/actions/workflows/run_tests.yml)

Welcome to Zed, a high-performance, multiplayer code editor from the creators of [Atom](https://github.com/atom/atom) and [Tree-sitter](https://github.com/tree-sitter/tree-sitter).

---

### Installation

On macOS, Linux, and Windows you can [download Zed directly](https://zed.dev/download) or install Zed via your local package manager ([macOS](https://zed.dev/docs/installation#macos)/[Linux](https://zed.dev/docs/linux#installing-via-a-package-manager)/[Windows](https://zed.dev/docs/windows#package-managers)).

Other platforms are not yet available:

- Web ([tracking discussion](https://github.com/zed-industries/zed/discussions/26195))

### Developing Zed

- [Building Zed for macOS](./docs/src/development/macos.md)
- [Building Zed for Linux](./docs/src/development/linux.md)
- [Building Zed for Windows](./docs/src/development/windows.md)

### Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md) for ways you can contribute to Zed.

Also... we're hiring! Check out our [jobs](https://zed.dev/jobs) page for open roles.

### Licensing

Zed source code is licensed primarily under GPL-3.0-or-later, with Apache-2.0 components where marked.

License information for third party dependencies must be correctly provided for CI to pass.

We use [`cargo-about`](https://github.com/EmbarkStudios/cargo-about) to automatically comply with open source licenses. If CI is failing, check the following:

- Is it showing a `no license specified` error for a crate you've created? If so, add `publish = false` under `[package]` in your crate's Cargo.toml.
- Is the error `failed to satisfy license requirements` for a dependency? If so, first determine what license the project has and whether this system is sufficient to comply with this license's requirements. If you're unsure, ask a lawyer. Once you've verified that this system is acceptable add the license's SPDX identifier to the `accepted` array in `script/licenses/zed-licenses.toml`.
- Is `cargo-about` unable to find the license for a dependency? If so, add a clarification field at the end of `script/licenses/zed-licenses.toml`, as specified in the [cargo-about book](https://embarkstudios.github.io/cargo-about/cli/generate/config.html#crate-configuration).

## Sponsorship

Zed is developed by **Zed Industries, Inc.**, a for-profit company.

If you’d like to financially support the project, you can do so via GitHub Sponsors.
Sponsorships go directly to Zed Industries and are used as general company revenue.
There are no perks or entitlements associated with sponsorship.
