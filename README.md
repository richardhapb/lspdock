# LSProxy

LSProxy is a lightweight Language Server Protocol (LSP) proxy designed to facilitate communication between IDEs and LSP servers. It supports dynamic path redirection, Docker integration, and configuration-based customization. LSProxy ensures seamless communication between your IDE and LSP server, even when the server is running inside a Docker container.

```mermaid
flowchart TD
    subgraph Without LSProxy
        IDE -.->|Path mismatch| LSP
        LSP["LSP Server (Inside Docker)"] -->|Path mismatch| IDE["IDE (Client)"]
    end

    subgraph With LSProxy
        IDE2["IDE (Client)"] --> LSProxy["LSProxy (Proxy)"]
        LSProxy["LSProxy (Proxy)"] -->|Path redirection| IDE2["IDE (Client)"]
        LSP2["LSP Server (Inside Docker)"] --> LSProxy
        LSProxy -.->|Path redirection| LSP2
    end
```

### Explanation:
1. **Without LSProxy**:
   - The IDE communicates directly with the LSP server inside Docker.
   - Path mismatches between the host and container can cause issues, breaking the communication.

2. **With LSProxy**:
   - LSProxy acts as a middle layer between the IDE and the LSP server.
   - LSProxy dynamically redirects paths, ensuring seamless communication between the IDE and the LSP server inside Docker.

---

## Features

- **Docker Integration**: Supports running LSP servers inside Docker containers.
- **Dynamic Path Redirection**: Automatically adjusts paths between host and container environments.
- **Match container environment**: If a method like `textDocument/definition` points to a third-party library inside a container, that file will be cloned into the local environment, allowing the IDE to navigate to it.
- **Configurable Variables**: Customize paths and behavior using environment variables and configuration files.
- **Logging**: Detailed logs for debugging and monitoring.

---

## Installation

Download the release for your system, or build it from source.

## Build from source

### Prerequisites

- **Rust**: Ensure you have Rust installed. You can install it using [rustup](https://rustup.rs/).

### Steps

1. Clone the repository:
   ```bash
   git clone https://github.com/richardhapb/lsproxy.git
   cd lsproxy
   ```

2. Build the project:
   ```bash
   cargo build --release
   ```

3. Install the binary:
   ```bash
   cp target/release/lsproxy /usr/local/bin/
   ```

---

## Configuration

LSProxy uses the following configuration hierarchy: if the top configuration file is present, use that configuration. Use one configuration file at a time. If an option is not present and the next config file contains it, that option will not be used.

```
<project-directory>/lsproxy.toml
~/.config/lsproxy/lsproxy.toml
```

### Example Configuration

```toml
# Path to the Docker container
container = "my-container"

# Path inside the Docker container
docker_internal_path = "/usr/src/app"

# Path on the host machine
local_path = "/Users/richard/dev/project"

# Executable for the LSP server
executable = "rust-analyzer"

# Pattern to determine if Docker should be used
pattern = "/usr/src/app"

# Optional: Controls PID handling for LSP servers that track client processes
# Set to true if your LSP server auto-terminates when it can't detect the client process
# For example: true for Pyright, false for Ruff LSP
patch_pid = true

# Optional: Log level; default is info
log_level = "debug"
```

If the pattern is not present in the current working directory, the proxy acts as the target LSP, without changing any messages, and redirects it directly. Also, the logs of the messages continue to be captured and written to the log file.

### PID Patching Explained

Some LSP servers attempt to monitor the client's process ID (PID) and automatically terminate when they can't detect the client. This behavior can cause problems in containerized environments where PIDs don't match between the host and container.

- **When to use `patch_pid = true`**:
  - If your LSP server unexpectedly terminates during use
  - For servers like Pyright that actively track the client process
  - When using Docker, where the host PID is not visible inside the container

- **When to use `patch_pid = false` or omit**:
  - For LSP servers that don't monitor the client process
  - For servers like Ruff LSP that don't auto-terminate
  - When running LSP servers locally (not in containers)

When `patch_pid = true`, LSProxy will:
1. Remove the PID from requests to the LSP server
2. Monitor the editor's process itself
3. Properly shut down the LSP server when you close your editor

This feature ensures a smooth experience with LSP servers that would otherwise terminate prematurely when they can't detect your editor's process.

### Available Variables

LSProxy supports dynamic variables that can be used in the configuration file:

- **`$CWD`**: Current working directory.
- **`$PARENT`**: Parent directory of the current working directory. For example, `/path/to/project`, where $PARENT resolves to `project`.
- **`$HOME`**: Home directory of the user.

These variables will be automatically expanded when LSProxy reads the configuration file.

#### Example with Variables

```toml
container = "$PARENT-container"
docker_internal_path = "/usr/src/app"
local_path = "$HOME/dev/project"
executable = "rust-analyzer"  # The binary should be in the PATH; otherwise, indicate the absolute path.
pattern = "$HOME/dev"
```

---

## Usage

Refer to the [IDEs configuration guide](ides.md) for detailed configuration steps.

### Running LSProxy

1. Start LSProxy:
   ```bash
   lsproxy
   ```

2. LSProxy will automatically read the configuration file and start the LSP server. If the `pattern` matches the current working directory, LSProxy will use Docker; otherwise, it will run the LSP server directly.

### Logs

Logs are written to a temporary directory. On Unix systems, this is located at `/tmp/lsproxy_trace.log`, and on Windows, it is located at `C:/Windows/Temp`. You can monitor the logs for debugging:

```bash
tail -f /tmp/lsproxy_trace.log
```

---

## Road Map

- [x] Generate the configuration hierarchy
- [x] Handle navigating LSP response like `textDocument/definition` in the local environment
- [x] Redirect URIs between Docker container and Host environment
- [x] Implement PID monitoring for the IDE
- [ ] Use two LSPs in the same project

---

## Troubleshooting

### Common Issues

1. **Configuration File Not Found**:
   Ensure the configuration file exists at `~/.config/lsproxy/lsproxy.toml`.

2. **Docker Not Found**:
   Install Docker and ensure the target container is running.

---

## Contributing

Contributions are welcome! Feel free to open issues or submit pull requests.

---

## License

This project is licensed under the MIT License.
