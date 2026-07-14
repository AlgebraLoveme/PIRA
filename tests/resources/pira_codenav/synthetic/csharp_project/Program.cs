using System;
using Pira.Models;

namespace Pira.App;

public static class Program
{
    private static readonly User DefaultUser = new("Ada");

    public static void Main()
    {
        Console.WriteLine(DefaultUser.Label);
    }
}
