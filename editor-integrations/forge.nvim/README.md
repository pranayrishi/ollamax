# forge.nvim

Minimal Neovim plugin for [Ollama-Forge](https://github.com/pranayrishi/ollamax).
Shells out to the `forge` CLI you already have installed — no LSP, no
language servers, no extra processes. The CLI does all the work; this
plugin is glue.

## Why

`forge` already has streaming output, deterministic replay, persistent
rules, free research tools, and a tool-using agent. The thing it's
missing is a way to call any of those from inside an editor without
dropping to a terminal. That's all this plugin does.

## Features

- `:Forge research <q>` — runs `forge research` in a split, streams the
  answer into the buffer as it arrives.
- `:Forge chat <q>` — same idea but `forge chat`.
- `:Forge runskill <name> <task>` — runs `forge run-skill`.
- `:Forge audit` — `forge audit` on the current working directory,
  writes findings to a quickfix list.
- `:Forge build <task>` — pipes `forge build` output into a scratch
  buffer in the project directory; you decide whether to keep it.
- `:Forge` — opens a picker over all available commands.

Every command is a thin shell-out so the plugin works on any Neovim
≥0.9 with no plugin manager dependencies. The output streams in real
time because we use `vim.fn.jobstart` with `on_stdout`.

## Install

### lazy.nvim

```lua
{
  "pranayrishi/ollamax",
  -- Point lazy at the editor-integrations subdirectory.
  dir = vim.fn.stdpath("data") .. "/lazy/ollamax/editor-integrations/forge.nvim",
  config = function()
    require("forge").setup({
      cmd = "forge",  -- override if your forge is elsewhere
    })
  end,
}
```

### packer.nvim

```lua
use {
  "pranayrishi/ollamax",
  rtp = "editor-integrations/forge.nvim",
  config = function() require("forge").setup() end,
}
```

### Manual

Symlink `lua/forge` and `plugin/forge.lua` into your Neovim runtime
path:

```bash
mkdir -p ~/.config/nvim/lua ~/.config/nvim/plugin
ln -s "$(pwd)/lua/forge"        ~/.config/nvim/lua/forge
ln -s "$(pwd)/plugin/forge.lua" ~/.config/nvim/plugin/forge.lua
```

Then in your `init.lua`: `require("forge").setup()`.

## Configuration

```lua
require("forge").setup({
  -- Command name or path. Defaults to "forge" on $PATH.
  cmd = "forge",
  -- Directory to spawn forge in. Defaults to vim.fn.getcwd().
  cwd = nil,
  -- Where streamed output lands. Either "split" (default) or "vsplit".
  output = "split",
  -- Set FORGE_REPLAY_LOG before each invocation so every editor session
  -- is automatically replayable. Set to a path to enable, or false.
  replay_log = false,
})
```

## Requirements

- Neovim ≥ 0.9 (uses `vim.fn.jobstart` with channel callbacks)
- `forge` CLI on `$PATH` (or its absolute path passed via `cmd`)
- Ollama running locally (forge talks to it)

## Status

Pre-alpha, like the rest of forge. The CLI is the source of truth; this
plugin is intentionally thin so a `forge` upgrade never breaks the
plugin.
