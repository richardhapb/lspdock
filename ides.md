# LSProxy Configuration for IDEs

This section explains how to configure LSProxy in popular IDEs: **Neovim**, **VSCode**, and **General Approach for Other IDEs**. The goal is to make the setup simple and straightforward.

`pyright` is used as an example, but any LSP can be proxied. Assuming this configuration:

```toml
container = "$PARENT-container"
docker_internal_path = "/usr/src/app"
local_path = "$HOME/dev/project"
executable = "pyright-langserver"
pattern = "$HOME/dev"
```

---

## Neovim

Steps for Neovim 0.11.0. For another version use [nvim-lspconfig](https://github.com/neovim/nvim-lspconfig) instead.

### Steps

1. Configure LSProxy in your Neovim configuration (`init.lua` or equivalent):
   ```lua
   vim.lsp.enable("lsproxy")
   ```

2. Create the file `lsproxy.lua` in `after/lsp` with the configuration of the LSP.

   ```lua
   return {
     cmd = { 'lsproxy', '--stdio' },
     -- Optional, by default the log level is INFO
     cmd_env = { RUST_LOG = "none,lsproxy=debug" },
     filetypes = { "python" },
     root_markers = {
       'pyproject.toml',
       'setup.py',
       'setup.cfg',
       'requirements.txt',
       'Pipfile',
       'pyrightconfig.json',
       '.git',
     },
     settings = {
       python = {
         analysis = {
           autoSearchPaths = true,
           useLibraryCodeForTypes = true,
           diagnosticMode = 'openFilesOnly',
         },
       },
     },
   }
   ```

3. Restart Neovim and open a Python project, in a path that matches the `pattern`, like `$HOME/dev/project`. LSProxy will act as a proxy for `pyright-langserver`.

---

## VSCode

### Prerequisites
- **LSP Extension**: Install the appropriate LSP extension for your language (e.g., Rust Analyzer, Pyright).

### Steps

1. Open VSCode and navigate to **Settings** > **Extensions** > **Rust Analyzer** (or the relevant LSP extension).

2. Update the **Server Path** setting to:
   ```
   lsproxy
   ```
3. Restart VSCode and open a project. LSProxy will act as a proxy for the LSP server.

---

## General Approach for Other IDEs

If your IDE supports configuring an external Language Server Protocol (LSP), you can use LSProxy by following these general steps:

### Prerequisites
- Ensure your IDE supports LSP configuration.
- Know the path to the LSProxy binary (e.g., `/usr/local/bin/lsproxy`).

### Steps

1. **Locate LSP Settings**:
   - Open your IDE's settings or preferences.
   - Navigate to the section for configuring Language Servers or LSP.

2. **Set the LSP Command**:
   - Replace the default LSP server command with `lsproxy`.
   - Example: If your IDE uses `rust-analyzer` as the default command, replace it with:
     ```
     lsproxy
     ```

3. **Pass Additional Arguments**:
   - If your IDE allows passing arguments to the LSP server, ensure the arguments are forwarded correctly. LSProxy will automatically forward them to the underlying LSP server.

4. **Configure LSProxy**:
   - Ensure LSProxy is configured correctly in `~/.config/lsproxy/lsproxy.toml`. For example:
     ```toml
     container = "my-container"
     docker_internal_path = "/usr/src/app"
     local_path = "/Users/richard/dev/project"
     executable = "rust-analyzer"
     pattern = "/usr/src/app"
     ```

5. **Restart the IDE**:
   - Restart your IDE to apply the changes.

6. **Verify Logs**:
   - Check `/tmp/lsproxy_trace.log` for detailed logs to ensure LSProxy is working correctly.

---

## Notes

- **Docker Integration**: If your LSP server runs inside a Docker container, ensure the `pattern` in the configuration file matches the current working directory. LSProxy will automatically detect whether to use Docker or run the LSP server directly.
- **Logs**: Check `/tmp/lsproxy_trace.log` for detailed logs if something isn't working as expected.

---

With these steps, LSProxy should work seamlessly with Neovim, VSCode, and other IDEs that support LSP. If you encounter issues, refer to the troubleshooting section in the README or open an issue on the LSProxy repository.

