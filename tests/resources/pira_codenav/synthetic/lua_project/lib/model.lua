local M = {}

local function normalize(value)
  return string.lower(value)
end

function M.new(name)
  local instance = { name = normalize(name) }

  function instance:label()
    return self.name
  end

  return instance
end

M.version = function()
  return "1"
end

return M
