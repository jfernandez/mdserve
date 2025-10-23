## [0.4.1] - 2025-10-23
### Features
- Add directory mode for serving multiple markdown files
- Add YAML and TOML frontmatter support
### Bug Fixes
- Center content in folder mode with sidebar collapsed
- Prevent 404 race during neovim saves
### Refactoring
- Simplify server startup output messages
- Migrate to minijinja template engine
### Documentation
- Update Cargo install instructions
- Update Arch linux install instructions to use official package
- Update README with new folder serving feature
### Build
- Downgrade to edition 2021 and set MSRV to 1.82.0
- Add git-cliff configuration
### CI
- Run on `aarch64-linux` as well
### Miscellaneous Tasks
- Add package metadata for cargo publish
- Remove macOS support, direct users to Homebrew

## [0.4.1] - 2025-10-23
### Bug Fixes
- Change default hostname to 127.0.0.1 to prevent port conflicts
### Documentation
- Update homebrew install instructions
- Release 0.4.1

## [0.4.0] - 2025-10-03
### Features
- Add ETag support for mermaid.min.js
### Refactoring
- Asref avoid clone
- Impl AsRef<Path>
### Documentation
- Add Arch Linux install instructions
### Build
- Optimize and reduce size of release binary (#8)
- Add nix flake packaging
- Update min Rust version to 1.85+ (2024)
- Bundle mermaid.min.js (#10)
- Remove cargo install instructions, add warning about naming conflict
- Add `-H|--hostname` to support listening on non-localhost
- Release 0.4.0

## [0.3.0] - 2025-09-27
- Prevent theme flash on page load
- Replace WebSocket content updates with reload signals (#4)
- Add mermaid diagram support (#5)
- Release 0.3.0

## [0.2.0] - 2025-09-24
- Add install script and update README
- Add macOS install instructions
- Add image support
- Add screenshot of mdserve serving README.md
- Enable HTML tag rendering in markdown files (#2)
- Release 0.2.0

## [0.1.0] - 2025-09-22
- Release 0.1.0

