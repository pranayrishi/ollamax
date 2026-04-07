-- forge.nvim — minimal Neovim glue around the `forge` CLI.
--
-- Design notes (for the next person reading this):
--
-- 1. We never embed forge logic. Every command shells out to the CLI via
--    `vim.fn.jobstart`. If forge gains a new flag, the plugin doesn't
--    need to change.
-- 2. Streaming uses `on_stdout` so users see tokens flow into the buffer
--    as they arrive — same UX as `forge chat` in a terminal. The cost
--    is one buffer write per chunk, which Neovim handles fine for the
--    output volumes a local LLM produces (~10-50 tok/s).
-- 3. There's no async runtime. `jobstart` is a callback API. Anything
--    fancier would mean dragging in plenary.nvim, which we don't need.

local M = {}

---@type table
M.config = {
  cmd = "forge",
  cwd = nil,
  output = "split",
  replay_log = false,
}

--- Merge user config over defaults. Idempotent — safe to call multiple times.
---@param opts table|nil
function M.setup(opts)
  if opts then
    for k, v in pairs(opts) do
      M.config[k] = v
    end
  end
end

--- Open a scratch buffer for streaming forge output. Returns the buffer
--- handle so callers can append to it.
---@param title string
---@return integer bufnr
local function open_output_buffer(title)
  local split_cmd = M.config.output == "vsplit" and "vnew" or "new"
  vim.cmd(split_cmd)
  local buf = vim.api.nvim_get_current_buf()
  vim.api.nvim_buf_set_option(buf, "buftype", "nofile")
  vim.api.nvim_buf_set_option(buf, "bufhidden", "wipe")
  vim.api.nvim_buf_set_option(buf, "swapfile", false)
  vim.api.nvim_buf_set_option(buf, "filetype", "markdown")
  vim.api.nvim_buf_set_name(buf, "forge://" .. title)
  return buf
end

--- Append `lines` to `buf` and scroll to the bottom of any window showing it.
local function append_lines(buf, lines)
  if not vim.api.nvim_buf_is_valid(buf) then
    return
  end
  -- jobstart's on_stdout sometimes hands us a partial trailing line; we
  -- accept it as-is. The next callback will continue it.
  local last_line = vim.api.nvim_buf_line_count(buf)
  vim.api.nvim_buf_set_lines(buf, last_line, last_line, false, lines)
  for _, win in ipairs(vim.api.nvim_list_wins()) do
    if vim.api.nvim_win_get_buf(win) == buf then
      vim.api.nvim_win_set_cursor(win, { vim.api.nvim_buf_line_count(buf), 0 })
    end
  end
end

--- Run `forge <args...>` and stream stdout into a fresh buffer.
---@param subcommand string                                 e.g. "research"
---@param args table                                        positional args + flags
---@param title string|nil                                  buffer title (defaults to subcommand)
function M.run(subcommand, args, title)
  local cmdline = { M.config.cmd, subcommand }
  for _, a in ipairs(args) do
    table.insert(cmdline, a)
  end

  local buf = open_output_buffer(title or subcommand)
  append_lines(buf, { "$ " .. table.concat(cmdline, " "), "" })

  local env = vim.fn.environ()
  if M.config.replay_log then
    env.FORGE_REPLAY_LOG = M.config.replay_log
  end
  -- jobstart wants env as { "KEY=VALUE", ... }, not a table.
  local env_list = {}
  for k, v in pairs(env) do
    table.insert(env_list, k .. "=" .. v)
  end

  vim.fn.jobstart(cmdline, {
    cwd = M.config.cwd or vim.fn.getcwd(),
    env = env_list,
    stdout_buffered = false,
    stderr_buffered = false,
    on_stdout = function(_, data, _)
      if data and #data > 0 then
        append_lines(buf, data)
      end
    end,
    on_stderr = function(_, data, _)
      if data and #data > 0 then
        -- Forge writes status (preload progress, agent steps, errors) to
        -- stderr; surface it inline so the user sees what's happening.
        local prefixed = {}
        for _, line in ipairs(data) do
          if line ~= "" then
            table.insert(prefixed, "│ " .. line)
          end
        end
        append_lines(buf, prefixed)
      end
    end,
    on_exit = function(_, code, _)
      append_lines(buf, { "", "[forge exited " .. code .. "]" })
    end,
  })
end

--- Convenience: read the user's input via vim.ui.input.
---@param prompt string
---@param cb fun(value: string|nil)
local function ask(prompt, cb)
  vim.ui.input({ prompt = prompt }, function(input)
    if input and input ~= "" then
      cb(input)
    end
  end)
end

function M.research(args)
  if args and #args > 0 then
    M.run("research", { args }, "research")
  else
    ask("Research question: ", function(q)
      M.run("research", { q }, "research")
    end)
  end
end

function M.chat(args)
  if args and #args > 0 then
    M.run("chat", { args }, "chat")
  else
    ask("Chat prompt: ", function(q)
      M.run("chat", { q }, "chat")
    end)
  end
end

function M.run_skill(skill_name, task)
  if not skill_name or skill_name == "" then
    ask("Skill name: ", function(s)
      ask("Task: ", function(t)
        M.run("run-skill", { s, t }, "run-skill:" .. s)
      end)
    end)
    return
  end
  if not task or task == "" then
    ask("Task: ", function(t)
      M.run("run-skill", { skill_name, t }, "run-skill:" .. skill_name)
    end)
    return
  end
  M.run("run-skill", { skill_name, task }, "run-skill:" .. skill_name)
end

function M.audit()
  M.run("audit", { vim.fn.getcwd() }, "audit")
end

function M.build(task)
  if task and task ~= "" then
    M.run("build", { task }, "build")
  else
    ask("Build task: ", function(t)
      M.run("build", { t }, "build")
    end)
  end
end

return M
