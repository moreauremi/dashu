-- Filtre Lua des documents convertis en PDF (src/tools/worker.rs).

-- Images. Pour un PDF, pandoc les laisse à Typst, qui ne lit que des fichiers
-- présents (et pandoc, en bac à sable, ne va rien chercher) : une image du
-- Markdown ferait échouer toute la conversion. Elle devient son texte de
-- remplacement, en italique.
function Image(img)
  if #img.caption == 0 then
    return pandoc.Emph({ pandoc.Str("[image]") })
  end
  return pandoc.Emph(img.caption)
end

-- Une image seule dans son paragraphe : son texte, sur sa propre ligne.
function Figure(fig)
  return pandoc.Para(pandoc.utils.blocks_to_inlines(fig.content))
end

-- Cases à cocher (« - [ ] » et « - [x] », que pandoc lit en ☐ et ☒) : une
-- vraie case, sans puce ; cochée, elle est bleue et le texte est barré, en
-- gris, comme dans Notion.
local BOX = '#box(baseline: 0.14em, width: 0.9em, height: 0.9em, radius: 2pt, '
local EMPTY = BOX .. 'stroke: 0.9pt + rgb("#37352f"))'
local CHECKED = BOX .. 'fill: rgb("#2383e2"), place(path(stroke: (paint: white, '
  .. 'thickness: 1.2pt, cap: "round", join: "round"), (0.2em, 0.47em), '
  .. '(0.38em, 0.66em), (0.7em, 0.26em))))'

local function task(item)
  local first = item[1]
  if not first or (first.t ~= "Plain" and first.t ~= "Para") then
    return nil
  end
  local mark = first.content[1]
  if not mark or mark.t ~= "Str" or (mark.text ~= "☐" and mark.text ~= "☒") then
    return nil
  end
  return mark.text == "☒"
end

function BulletList(list)
  for _, item in ipairs(list.content) do
    if task(item) == nil then
      return nil
    end
  end
  for _, item in ipairs(list.content) do
    local first = item[1]
    local done = task(item)
    local rest = pandoc.List(first.content)
    rest:remove(1)
    if rest[1] and rest[1].t == "Space" then
      rest:remove(1)
    end
    local text = pandoc.List({ pandoc.RawInline("typst", done and CHECKED or EMPTY),
      pandoc.RawInline("typst", "#h(0.55em)") })
    if done then
      text:insert(pandoc.RawInline("typst", '#text(fill: rgb("#787774"))[#strike['))
      text:extend(rest)
      text:insert(pandoc.RawInline("typst", "]]"))
    else
      text:extend(rest)
    end
    first.content = text
  end
  return {
    pandoc.RawBlock("typst", "#[\n#set list(marker: [], indent: 0pt, body-indent: 0pt)"),
    list,
    pandoc.RawBlock("typst", "]"),
  }
end
