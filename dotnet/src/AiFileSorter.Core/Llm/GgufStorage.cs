namespace AiFileSorter.Core.Llm;

/// <summary>Resolves the same default LLM storage directory the Qt app uses so downloads can be shared.</summary>
public static class GgufStorage
{
    public const string PartialSuffix = ".part";
    public const string MetaSuffix = ".aifs.meta";

    public static string DefaultDirectory()
    {
        var env = Environment.GetEnvironmentVariable("AI_FILE_SORTER_LLM_STORAGE_DIR")
                  ?? Environment.GetEnvironmentVariable("AI_FILE_SORTER_LLM_DIR");
        if (!string.IsNullOrWhiteSpace(env))
        {
            return env.Trim();
        }

        if (OperatingSystem.IsWindows())
        {
            var appData = Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData);
            return Path.Combine(appData, "aifilesorter", "llms");
        }

        var home = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        if (OperatingSystem.IsMacOS())
        {
            return Path.Combine(home, "Library", "Application Support", "aifilesorter", "llms");
        }

        return Path.Combine(home, ".local", "share", "aifilesorter", "llms");
    }

    public static string ResolveDirectory(string? overrideDirectory) =>
        string.IsNullOrWhiteSpace(overrideDirectory) ? DefaultDirectory() : overrideDirectory.Trim();

    public static string DestinationPath(GgufArtifact artifact, string? overrideDirectory) =>
        Path.GetFullPath(Path.Combine(ResolveDirectory(overrideDirectory), artifact.RelativePath));

    public static string PartialPath(string destinationPath) => destinationPath + PartialSuffix;

    public static string MetaPath(string destinationPath) => destinationPath + MetaSuffix;

    public static string? ResolveExistingPath(GgufArtifact artifact, string? overrideDirectory)
    {
        var preferred = DestinationPath(artifact, overrideDirectory);
        if (GgufFileValidation.HasGgufHeader(preferred))
        {
            return preferred;
        }

        if (!string.IsNullOrWhiteSpace(artifact.SharesUrlWithArtifactId))
        {
            var shared = GgufCatalog.FindArtifact(artifact.SharesUrlWithArtifactId);
            if (shared is not null)
            {
                var sharedPath = DestinationPath(shared, overrideDirectory);
                if (GgufFileValidation.HasGgufHeader(sharedPath) &&
                    string.Equals(artifact.ResolveUrl(), shared.ResolveUrl(), StringComparison.Ordinal))
                {
                    return sharedPath;
                }
            }
        }

        return null;
    }
}

public static class GgufFileValidation
{
    public static readonly byte[] Magic = "GGUF"u8.ToArray();

    public static bool IsGgufPath(string path) =>
        path.EndsWith(".gguf", StringComparison.OrdinalIgnoreCase);

    public static bool HasGgufHeader(string path)
    {
        if (!File.Exists(path))
        {
            return false;
        }

        try
        {
            using var stream = File.OpenRead(path);
            Span<byte> magic = stackalloc byte[4];
            var read = stream.Read(magic);
            return read == 4 && magic.SequenceEqual(Magic);
        }
        catch (IOException)
        {
            return false;
        }
    }

    public static void WriteTinyFixture(string path, int extraBytes = 32)
    {
        var directory = Path.GetDirectoryName(path);
        if (!string.IsNullOrWhiteSpace(directory))
        {
            Directory.CreateDirectory(directory);
        }

        var bytes = new byte[Magic.Length + extraBytes];
        Magic.CopyTo(bytes, 0);
        File.WriteAllBytes(path, bytes);
    }
}
