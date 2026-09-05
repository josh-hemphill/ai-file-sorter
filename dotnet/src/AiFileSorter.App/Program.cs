using System;
using Avalonia;
using AiFileSorter.Core.Persistence;

namespace AiFileSorter.App;

internal static class Program
{
    [STAThread]
    public static void Main(string[] args)
    {
        NativeSqlite.EnsureInitialized();
        BuildAvaloniaApp().StartWithClassicDesktopLifetime(args);
    }

    public static AppBuilder BuildAvaloniaApp()
        => AppBuilder.Configure<App>()
            .UsePlatformDetect()
            .WithInterFont()
            .LogToTrace();
}
