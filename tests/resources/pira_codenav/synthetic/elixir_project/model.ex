defmodule Demo.User do
  import String

  defstruct [:name]

  def new(name), do: %__MODULE__{name: name}
  def label(%__MODULE__{} = user), do: user.name
  defp normalize(value), do: trim(value)
end

defprotocol Demo.Labelled do
  def label(value)
end
