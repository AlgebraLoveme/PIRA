namespace Pira.Models;

public sealed record User(string Name)
{
    public string Label => Name.Trim();
}
