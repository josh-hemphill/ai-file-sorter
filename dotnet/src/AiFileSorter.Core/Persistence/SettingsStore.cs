using AiFileSorter.Core.Json;
using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Persistence;

/// <summary>Loads and saves LLM / content settings next to the suggestion database.</summary>
public static class SettingsStore
{
    public static string DefaultPath()
    {
        var root = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
        if (string.IsNullOrWhiteSpace(root))
        {
            root = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".local", "share");
        }

        return Path.Combine(root, "AIFileSorter", "avalonia", "app-settings.json");
    }

    public static AppSettings Load(string? path = null)
    {
        var file = path ?? DefaultPath();
        if (!File.Exists(file))
        {
            return new AppSettings();
        }

        try
        {
            return AppJson.Deserialize<AppSettings>(File.ReadAllText(file));
        }
        catch (System.Text.Json.JsonException)
        {
            return new AppSettings();
        }
    }

    public static void Save(AppSettings settings, string? path = null)
    {
        var file = path ?? DefaultPath();
        var directory = Path.GetDirectoryName(file);
        if (!string.IsNullOrWhiteSpace(directory))
        {
            Directory.CreateDirectory(directory);
        }

        File.WriteAllText(file, AppJson.Serialize(settings));
    }
}
