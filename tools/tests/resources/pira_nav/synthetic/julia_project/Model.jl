module Demo

using Dates
import Base: show

export User, normalize

abstract type Entity end
primitive type Token 32 end

struct User <: Entity
    name::String
end

function normalize(value::String)
    strip(value)
end

double(x) = 2x
Base.show(io::IO, user::User) = print(io, user.name)

macro traced(ex)
    esc(ex)
end

end
