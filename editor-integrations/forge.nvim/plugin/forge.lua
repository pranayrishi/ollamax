-- forge.nvim — user commands.
--
-- This file is loaded automatically by Neovim's plugin loader. The
-- runtime entrypoint (`require("forge").setup()`) does the actual work
-- of merging user config; this file just registers the `:Forge*`
-- commands so they're available immediately.

if vim.g.loaded_forge then
  return
end
vim.g.loaded_forge = 1

-- All the subcommands. Keeping the dispatch table here (not in
-- lua/forge/init.lua) so the plugin file can be ridiculously thin.
local function dispatch(opts)
  local forge = require("forge")
  local sub = opts.fargs[1]
  local rest = {}
  for i = 2, #opts.fargs do
    table.insert(rest, opts.fargs[i])
  end
  local joined = table.concat(rest, " ")

  if sub == "research" then
    forge.research(joined)
  elseif sub == "chat" then
    forge.chat(joined)
  elseif sub == "runskill" then
    forge.run_skill(rest[1], table.concat({ unpack(rest, 2) }, " "))
  elseif sub == "audit" then
    forge.audit()
  elseif sub == "build" then
    forge.build(joined)
  elseif sub == nil or sub == "" then
    -- Picker over all subcommands.
    vim.ui.select(
      { "research", "chat", "runskill", "audit", "build" },
      { prompt = "forge: " },
      function(choice)
        if choice then
          dispatch({ fargs = { choice } })
        end
      end
    )
  else
    vim.notify("forge.nvim: unknown subcommand `" .. sub .. "`", vim.log.levels.ERROR)
  end
end

vim.api.nvim_create_user_command("Forge", dispatch, {
  nargs = "*",
  complete = function(_, line)
    local n = #vim.split(line, "%s+")
    if n <= 2 then
      return { "research", "chat", "runskill", "audit", "build" }
    end
    return {}
  end,
  desc = "Run an Ollama-Forge command (research, chat, runskill, audit, build)",
})
