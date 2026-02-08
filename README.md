# Workforest

Workforest is a terminal UI for managing local “agents” tied to git repositories. It runs a small local server that manages agent worktrees and sessions, and a TUI that connects to the server to browse, create, and control agents.

## Components

- `workforest` (CLI): launches the server and TUI, and can stop the server.
- `workforest-server`: local HTTP API for repos and agents.
- `workforest-tui`: terminal UI that talks to the server.
- `workforest-core`: shared types and config helpers.

## Requirements

- Rust toolchain (stable)

## Build

```bash
cargo build
```

## Run

Start the app (CLI launches the server and TUI):

```bash
cargo run -p workforest
```

Stop the server:

```bash
cargo run -p workforest -- stop-server
```

The server writes its port metadata under the app config directory.

## Configuration

Repos are stored in `repos.toml` under the config directory for your OS. The file is created and updated via the TUI when you add repositories.

## License

See `LICENSE.md`.
