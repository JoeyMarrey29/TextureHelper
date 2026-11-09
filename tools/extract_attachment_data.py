"""Generate ui/attachment-data.js from decoded PERPHEADS client Lua.

Usage:
    python tools/extract_attachment_data.py <decoded-lua-dir> [ui/attachment-data.js]

The input is the folder written by gmoddem's gluapack export. The attachment
definitions are executed with a small stub environment (requires `pip install lupa`)
so helper constructors, shared requirement tables, and overrides behave exactly
as they do in game.
"""

import glob
import json
import os
import re
import sys

try:
    from lupa import lua54 as lupa_runtime
except ImportError:
    import lupa as lupa_runtime

MODULE = "gamemodes/darkrp/gamemode/modules"
WEAPONSYSTEM = MODULE + "/weaponsystem"
ATTACHMENT_FILES = ["magazines", "other", "rails", "sights", "stocks", "suppressors"]
SLOT_LABELS = {
    "SLOT_SIGHT": "Sight",
    "SLOT_STOCK": "Stock",
    "SLOT_SILENCER": "Muzzle",
    "SLOT_UNDERBARREL": "Underbarrel",
    "SLOT_LEFT_RAIL": "Laser",
    "SLOT_MAGAZINE": "Magazine",
    "SLOT_SCOPE_RINGS": "Scope rings",
    "SLOT_TOP_RAIL": "Top rail",
    "SLOT_IRON_FRONT": "Front iron sight",
    "SLOT_IRON_REAR": "Rear iron sight",
    "SLOT_BARREL": "Barrel",
}

PRELUDE = r"""
AddCSLuaFile = function() end
CLIENT, SERVER = false, false
timer = { Simple = function() end }
hook = { Add = function() end, Run = function() end }
Vector = function(...) return { ... } end
Angle = Vector
Either = function(condition, a, b) if condition then return a end return b end
MsgC = function() end
ErrorNoHalt = function() end
string.StartWith = function(value, prefix) return value:sub(1, #prefix) == prefix end
table.HasValue = function(tbl, value)
    for _, item in pairs(tbl) do if item == value then return true end end
    return false
end
table.toStream = function(tbl)
    local stream = { items = tbl }
    function stream:map(fn)
        local out = {}
        for i, item in ipairs(self.items) do out[i] = fn(item) end
        return table.toStream(out)
    end
    function stream:toTable() return self.items end
    return stream
end
"""

CAPTURE = r"""
__created = {}
local originalNew = GunAttachment.New
function GunAttachment:New(name, model, slot)
    local attachment = originalNew(self, name, model, slot)
    __created[#__created + 1] = attachment
    return attachment
end
"""


def read_text(path):
    data = open(path, "rb").read()
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        return data.decode("cp1252")


def glua_to_lua(source):
    """Translate the Garry's Mod Lua operators used by these files to standard Lua."""
    out, i, n = [], 0, len(source)
    while i < n:
        c, two = source[i], source[i:i + 2]
        if two == "--":
            long = re.match(r"--\[(=*)\[", source[i:])
            if long:
                close = source.find("]" + long.group(1) + "]", i)
                end = n if close < 0 else close + len(long.group(1)) + 2
            else:
                end = source.find("\n", i)
                end = n if end < 0 else end
            out.append(source[i:end]); i = end
        elif two == "//":
            end = source.find("\n", i); end = n if end < 0 else end
            out.append("--" + source[i + 2:end]); i = end
        elif two == "/*":
            end = source.find("*/", i); end = n if end < 0 else end + 2
            out.append("--[==[" + source[i + 2:end - 2] + "]==]"); i = end
        elif c in "\"'":
            j = i + 1
            while j < n and source[j] != c:
                j += 2 if source[j] == "\\" else 1
            out.append(source[i:j + 1]); i = j + 1
        elif c == "[" and re.match(r"\[=*\[", source[i:]):
            level = re.match(r"\[(=*)\[", source[i:]).group(1)
            close = source.find("]" + level + "]", i)
            end = n if close < 0 else close + len(level) + 2
            out.append(source[i:end]); i = end
        elif two == "!=":
            out.append("~="); i += 2
        elif c == "!":
            out.append(" not "); i += 1
        elif two == "&&":
            out.append(" and "); i += 2
        elif two == "||":
            out.append(" or "); i += 2
        elif re.match(r"continue\b", source[i:]) and (i == 0 or not (source[i - 1].isalnum() or source[i - 1] == "_")):
            # Only used in a deferred timer callback that the extractor never runs.
            out.append("do end"); i += len("continue")
        else:
            out.append(c); i += 1
    return "".join(out)


def lua_list(table):
    return [value for _, value in sorted(table.items())] if table is not None else []


def load_attachments(root):
    lua = lupa_runtime.LuaRuntime(unpack_returned_tuples=True)
    lua.execute(PRELUDE)
    lua.execute(glua_to_lua(read_text(os.path.join(root, WEAPONSYSTEM, "sh_attachments.lua"))))
    lua.execute(CAPTURE)
    for name in ATTACHMENT_FILES:
        lua.execute(glua_to_lua(read_text(os.path.join(root, WEAPONSYSTEM, "attachments", name + ".lua"))))
    gun_attachment = lua.globals().GunAttachment
    slot_names = {int(value): key for key, value in gun_attachment.items() if str(key).startswith("SLOT_")}
    sequences = lua.eval("""
        function(attachment)
            CLIENT, SERVER = true, false
            IsValid = function(entity) return entity ~= nil end
            local function sample(attachments)
                local entity = {}
                function entity:SetSequence(sequence) self.sequence = sequence end
                attachment:OnAttachmentAdded(nil, attachments, entity)
                return entity.sequence
            end
            local withoutSight = sample({})
            local withSight = sample({ { Slot = GunAttachment.SLOT_SIGHT } })
            CLIENT, SERVER = false, false
            if withoutSight ~= nil or withSight ~= nil then
                assert(withoutSight ~= nil and withSight ~= nil, "Incomplete attachment sequence rule")
                return withoutSight, withSight
            end
        end
    """)
    attachments = {}
    for _, item in sorted(lua.globals().__created.items()):
        compatible = sorted(str(key)[len("weapon_perp_phys_"):] for key in item.CompatibleGuns.keys())
        requirements = {str(gun): lua_list(names) for gun, names in item.RequiredAttachmentNames.items()}
        attachments[item.Name] = {
            "slot": slot_names[int(item.Slot)],
            "model": item.Model.replace("\\", "/").lower(),
            "compatible": compatible,
            "requirements": requirements,
        }
        sequence_rule = sequences(item)
        if sequence_rule is not None:
            without_sight, with_sight = sequence_rule
            attachments[item.Name]["sequences"] = {
                "withoutSight": int(without_sight), "withSight": int(with_sight),
            }
    return gun_attachment, attachments


def parse_translations(root):
    names = {}
    for path in glob.glob(os.path.join(root, MODULE, "translation", "en", "**", "*.lua"), recursive=True):
        for key, value in re.findall(r'\{"([^"]+)",\s*"((?:[^"\\]|\\.)*)"\}', read_text(path)):
            names[key] = value
    return names


def parse_items(root, translations):
    items = {}
    for file_name, base in (("attachments.lua", "base_attachment"), ("magazines.lua", "base_clip")):
        text = read_text(os.path.join(root, MODULE, "itemsystem", "items", file_name))
        for block in re.split(r"(?=\b%s\(\")" % base, text)[1:]:
            name = re.match(r'%s\("([^"]+)"\)' % base, block).group(1)
            item_id = re.search(r"ITEM\.ID\s*=\s*(\d+)", block)
            label = re.search(r'ITEM\.Name\s*=\s*T\("([^"]+)"\)', block)
            if item_id:
                items[name] = {"id": int(item_id.group(1)), "label": translations.get(label.group(1)) if label else None}
    return items


def parse_weapons(root, translations):
    items, view_models = {}, {}
    for path in glob.glob(os.path.join(root, "**", "*.lua"), recursive=True):
        text = read_text(path)
        weapon_class = re.search(r'weaponclass\s*=\s*"weapon_perp_phys_(\w+)"', text)
        item_id = re.search(r"ITEM\.ID\s*=\s*(\d+)", text)
        if weapon_class and item_id:
            label = re.search(r'ITEM\.Name\s*=\s*T\("([^"]+)"\)', text)
            items[weapon_class.group(1)] = (int(item_id.group(1)), translations.get(label.group(1)) if label else None)
        swep_id = re.search(r"SWEP\.ID\s*=\s*(\d+)", text)
        view_model = re.search(r'SWEP\.ViewModel\s*=\s*"([^"]+)"', text)
        if swep_id and view_model:
            view_models[int(swep_id.group(1))] = view_model.group(1).replace("\\", "/").lower()
    return {key: (item_id, label, view_models[item_id]) for key, (item_id, label) in items.items() if item_id in view_models}


def humanize(name):
    return name.replace("_", " ").capitalize()


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    root = sys.argv[1]
    output = sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.path.dirname(__file__), "..", "ui", "attachment-data.js")
    gun_attachment, attachments = load_attachments(root)
    translations = parse_translations(root)
    items = parse_items(root, translations)
    weapons = parse_weapons(root, translations)

    for name, attachment in attachments.items():
        item = items.get(name)
        attachment["itemId"] = item["id"] if item else None
        attachment["label"] = (item and item["label"]) or humanize(name)

    weapon_data = {}
    for key, (item_id, label, view_model) in sorted(weapons.items()):
        defaults = [str(entry.Name) for entry in lua_list(gun_attachment.GetDefaultAttachments(gun_attachment, key))]
        # GetCompatibleAttachments only offers attachments that are backed by an inventory item.
        compatible = sorted(
            name for name, attachment in attachments.items()
            if key in attachment["compatible"] and attachment["itemId"] is not None
        )
        seen = set(compatible) | set(defaults)
        pending, requirements = list(seen), {}
        while pending:
            name = pending.pop()
            needed = attachments[name]["requirements"].get(key)
            if not needed:
                continue
            requirements[name] = needed
            for requirement in needed:
                if requirement not in seen:
                    seen.add(requirement)
                    pending.append(requirement)
        weapon_data[view_model] = {
            "key": key,
            "itemId": item_id,
            "name": label or key,
            "compatible": compatible,
            "defaults": defaults,
            "requirements": dict(sorted(requirements.items())),
        }

    for attachment in attachments.values():
        del attachment["compatible"], attachment["requirements"]

    slots = sorted({a["slot"] for a in attachments.values()}, key=list(SLOT_LABELS).index)
    data = {
        "source": "PERPHEADS gamemode Lua: modules/weaponsystem/sh_attachments.lua, attachments/*.lua, itemsystem item registries, and weapon SWEP definitions.",
        "slots": {slot: SLOT_LABELS[slot] for slot in slots},
        "attachments": dict(sorted(attachments.items())),
        "weapons": weapon_data,
    }
    text = (
        "// Generated by tools/extract_attachment_data.py from decoded PERPHEADS client Lua. Do not edit by hand.\n"
        "export const attachmentData = " + json.dumps(data, indent=1, ensure_ascii=False) + ";\n"
    )
    with open(output, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)
    print(f"{len(weapon_data)} weapons, {len(attachments)} attachments -> {os.path.abspath(output)}")


if __name__ == "__main__":
    main()
