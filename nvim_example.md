# Neovim LSP config examples

## Running LSP using the config file

Using `LSPDock` with neovim is very simple. Using Neovim >= 0.11 define this in one part of your `init.lua` and name it appropriately. For using a default config, ensure that `lspdock.toml` is in `~/.config/lspdock` or in the project root.

```lua
vim.lsp.enable("lspdock")
```

Then in your neovim config root: `after/lsp/lspdock.lua`. Notice that the name is the same as defined previously with `lspdock` in `vim.lsp.enable`. For example for `pyright-langserver`:

```lua
  -- after/lsp/lspdock.lua
return {
  -- Run the LSP normally but replace `ruff` with `lspdock`
  cmd = { "lspdock", "--stdio" },

  -- Normal config
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
  on_attach = lsp_utils.on_attach,
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

For this example this config works fine, for mas info about the config see the [README](README.md).

```toml
container = "$PARENT-web-1"
local_path = "$CWD/app"
docker_internal_path = "/usr/src/app"
executable = "pyright-langserver"
pattern = "$HOME/dev/"
patch_pid = ["pyright-langserver"]
```

## Working with multiples LSPs in the same container

The only one difference for using multiples LSPs in the same container is passing the `--exec` argument to indicate the binary to use, this allows to `LSPDock` figure out what LSP the IDE is trying to attach to.

Enable the LSP in `init.lua`.

```lua
vim.lsp.enable("lspdock_ruff")
```

Configure it in `after/lsp/lspdock_ruff.lua`.

```lua
return {
  -- This is the important part
  cmd = { "lspdock", "--exec", "ruff", 'server' },
  -- Optional for verbose logs
  cmd_env = { RUST_LOG = "none,lsdock=trace" },

  -- Normal LSP's config
  filetypes = { 'python' },
  root_markers = {
    'pyproject.toml',
    'ruff.toml',
    '.ruff.toml',
    'setup.py',
    'setup.cfg',
    'requirements.txt',
    'Pipfile',
    'pyrightconfig.json',
    '.git',
  },
  single_file_support = true,
  settings = {
    trace = "messages",
  },
}
```

For `pyright` a similar approach.

```lua
vim.lsp.enable("lspdock_pyright")
```

Then

```lua
--- lspdock_pyright.lua
return {
  -- This is the important part
  cmd = { "lspdock", "--exec", "pyright-langserver", '--stdio' },

  -- Normal config
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
  on_attach = lsp_utils.on_attach,
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

