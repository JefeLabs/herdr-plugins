//! The Neovim side. One Neovim serves both the formatted view and editing,
//! so an agent opening a file never costs unsaved work: the view is a
//! terminal buffer running `herdr-doc view` (`less` over the renderer), and
//! editing is the file's own buffer. Paths reach this Lua as data, through
//! `v:lua` calls with a quoted string, and become commands only through
//! `nvim_cmd` with filename and bar magic off.

use std::path::Path;

/// The session's document view: `HerdrDoc.view(file)`, `HerdrDoc.edit(file)`,
/// `:Doc [file]` (view a file, or toggle between a view and its file), and
/// `<leader>v` for `:Doc` when it is free. `HERDR_DOC_BIN` names herdr-doc.
pub const LUA: &str = r#"
HerdrDoc = HerdrDoc or {}
local bin = vim.env.HERDR_DOC_BIN or 'herdr-doc'

local function open_term(cmd)
  if vim.fn.has('nvim-0.11') == 1 then
    return vim.fn.jobstart(cmd, { term = true })
  end
  return vim.fn.termopen(cmd)
end

-- the formatted view of `file`, in place of any earlier view of it
function HerdrDoc.view(file)
  vim.schedule(function()
    file = vim.fn.fnamemodify(file, ':p')
    for _, b in ipairs(vim.api.nvim_list_bufs()) do
      if vim.api.nvim_buf_is_valid(b) and vim.b[b].herdr_doc == file then
        pcall(vim.api.nvim_buf_delete, b, { force = true })
      end
    end
    vim.cmd('enew')
    local buf = vim.api.nvim_get_current_buf()
    open_term({ bin, 'view', file })
    vim.b[buf].herdr_doc = file
    vim.cmd('startinsert')
  end)
  return 1
end

-- `file` in its own buffer, to edit (the view stays behind)
function HerdrDoc.edit(file)
  vim.schedule(function()
    vim.cmd('stopinsert')
    vim.cmd({ cmd = 'edit', args = { file }, magic = { file = false, bar = false } })
  end)
  return 1
end

vim.api.nvim_create_user_command('Doc', function(o)
  if o.args ~= '' then
    return HerdrDoc.view(o.args)
  end
  local shown = vim.b.herdr_doc
  if shown then
    return HerdrDoc.edit(shown)
  end
  local name = vim.api.nvim_buf_get_name(0)
  if name == '' then
    vim.notify('Doc: this buffer has no file')
  else
    HerdrDoc.view(name)
  end
end, { nargs = '?', complete = 'file', desc = 'herdr-doc: view a file, or toggle view and edit' })

-- quitting the view (`q` in less) closes it
vim.api.nvim_create_autocmd('TermClose', {
  callback = function(a)
    if vim.b[a.buf].herdr_doc then
      vim.schedule(function()
        pcall(vim.api.nvim_buf_delete, a.buf, { force = true })
      end)
    end
  end,
})

if vim.fn.maparg('<leader>v', 'n') == '' then
  vim.keymap.set('n', '<leader>v', '<Cmd>Doc<CR>', { desc = 'herdr-doc: view / edit' })
end
"#;

/// What the pane starts Neovim with: listening on `socket`, with the
/// document view loaded after the user's own config.
pub fn args(socket: &Path) -> Vec<String> {
    vec![
        "--listen".into(),
        socket.display().to_string(),
        "-c".into(),
        format!("lua {LUA}"),
    ]
}
