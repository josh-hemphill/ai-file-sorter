using AiFileSorter.Core.Json;

namespace AiFileSorter.Core.Llm;

public sealed record GgufDownloadMeta
{
    public string Url { get; init; } = "";
    public long ContentLength { get; init; }
}

public sealed record GgufDownloadProgress
{
    public long BytesReceived { get; init; }
    public long? TotalBytes { get; init; }
    public string Status { get; init; } = "";
    public double Fraction => TotalBytes is > 0 ? Math.Clamp((double)BytesReceived / TotalBytes.Value, 0, 1) : 0;
}

public sealed record GgufProbeResult
{
    public required GgufLocalState State { get; init; }
    public string Path { get; init; } = "";
    public string Url { get; init; } = "";
    public long BytesOnDisk { get; init; }
    public long? ExpectedBytes { get; init; }
    public string Message { get; init; } = "";
}

/// <summary>Resumable GGUF downloader matching the Qt LLMDownloader contract (.part + .aifs.meta + GGUF magic).</summary>
public sealed class GgufDownloader : IDisposable
{
    private readonly HttpMessageHandler _handler;
    private readonly bool _disposeHandler;

    public GgufDownloader()
        : this(new SocketsHttpHandler { AllowAutoRedirect = true, PooledConnectionLifetime = TimeSpan.FromMinutes(5) }, disposeHandler: true)
    {
    }

    public GgufDownloader(HttpMessageHandler handler, bool disposeHandler = false)
    {
        _handler = handler;
        _disposeHandler = disposeHandler;
    }

    public GgufProbeResult Probe(GgufArtifact artifact, string? storageDir)
    {
        var url = artifact.ResolveUrl();
        var destination = GgufStorage.DestinationPath(artifact, storageDir);
        if (string.IsNullOrWhiteSpace(url))
        {
            return new GgufProbeResult { State = GgufLocalState.MissingUrl, Path = destination, Message = $"Missing download URL environment variable ({artifact.UrlEnv})." };
        }

        if (GgufFileValidation.HasGgufHeader(destination))
        {
            return new GgufProbeResult
            {
                State = GgufLocalState.Complete,
                Path = destination,
                Url = url,
                BytesOnDisk = FileSize(destination),
                Message = "Downloaded"
            };
        }

        var shared = GgufStorage.ResolveExistingPath(artifact, storageDir);
        if (!string.IsNullOrWhiteSpace(shared) && !string.Equals(shared, destination, StringComparison.OrdinalIgnoreCase))
        {
            return new GgufProbeResult
            {
                State = GgufLocalState.Complete,
                Path = shared,
                Url = url,
                BytesOnDisk = FileSize(shared),
                Message = "Available from a shared GGUF already on disk"
            };
        }

        if (File.Exists(destination) && GgufFileValidation.IsGgufPath(destination))
        {
            return new GgufProbeResult
            {
                State = GgufLocalState.Corrupt,
                Path = destination,
                Url = url,
                BytesOnDisk = FileSize(destination),
                Message = "Downloaded file is invalid or incomplete (expected GGUF header)."
            };
        }

        var partial = GgufStorage.PartialPath(destination);
        if (File.Exists(partial))
        {
            var meta = ReadMeta(destination);
            return new GgufProbeResult
            {
                State = GgufLocalState.Partial,
                Path = destination,
                Url = url,
                BytesOnDisk = FileSize(partial),
                ExpectedBytes = meta?.ContentLength > 0 ? meta.ContentLength : null,
                Message = "Partial download can be resumed"
            };
        }

        return new GgufProbeResult
        {
            State = GgufLocalState.NotStarted,
            Path = destination,
            Url = url,
            Message = "Not downloaded"
        };
    }

    public async Task DownloadAsync(
        GgufArtifact artifact,
        string? storageDir,
        IProgress<GgufDownloadProgress>? progress,
        CancellationToken cancellationToken = default)
    {
        var url = artifact.ResolveUrl();
        if (string.IsNullOrWhiteSpace(url))
        {
            throw new InvalidOperationException($"Missing download URL environment variable ({artifact.UrlEnv}).");
        }

        var destination = GgufStorage.DestinationPath(artifact, storageDir);
        var partial = GgufStorage.PartialPath(destination);
        Directory.CreateDirectory(Path.GetDirectoryName(destination)!);

        using var client = new HttpClient(_handler, disposeHandler: false)
        {
            Timeout = TimeSpan.FromHours(6)
        };
        client.DefaultRequestHeaders.UserAgent.ParseAdd("aifs-avalonia/0.1");

        var resumeOffset = File.Exists(partial) ? new FileInfo(partial).Length : 0L;
        using var request = new HttpRequestMessage(HttpMethod.Get, url);
        if (resumeOffset > 0)
        {
            request.Headers.Range = new System.Net.Http.Headers.RangeHeaderValue(resumeOffset, null);
        }

        progress?.Report(new GgufDownloadProgress { BytesReceived = resumeOffset, Status = "Connecting…" });
        using var response = await client.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, cancellationToken).ConfigureAwait(false);
        if (resumeOffset > 0 && response.StatusCode == System.Net.HttpStatusCode.OK)
        {
            resumeOffset = 0;
            if (File.Exists(partial))
            {
                File.Delete(partial);
            }
        }

        response.EnsureSuccessStatusCode();
        var total = response.Content.Headers.ContentLength;
        if (total is not null && resumeOffset > 0 && response.StatusCode == System.Net.HttpStatusCode.PartialContent)
        {
            total += resumeOffset;
        }

        WriteMeta(destination, new GgufDownloadMeta { Url = url, ContentLength = total ?? 0 });

        await using var input = await response.Content.ReadAsStreamAsync(cancellationToken).ConfigureAwait(false);
        await using var output = new FileStream(partial, resumeOffset > 0 ? FileMode.Append : FileMode.Create, FileAccess.Write, FileShare.Read);
        var buffer = new byte[64 * 1024];
        var received = resumeOffset;
        while (true)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var read = await input.ReadAsync(buffer, cancellationToken).ConfigureAwait(false);
            if (read == 0)
            {
                break;
            }

            await output.WriteAsync(buffer.AsMemory(0, read), cancellationToken).ConfigureAwait(false);
            received += read;
            progress?.Report(new GgufDownloadProgress
            {
                BytesReceived = received,
                TotalBytes = total,
                Status = total is > 0 ? $"Downloading {FormatSize(received)} / {FormatSize(total.Value)}" : $"Downloading {FormatSize(received)}"
            });
        }

        await output.FlushAsync(cancellationToken).ConfigureAwait(false);
        output.Dispose();

        if (!GgufFileValidation.HasGgufHeader(partial))
        {
            throw new InvalidOperationException("Downloaded file is invalid or incomplete (expected GGUF header): " + destination);
        }

        File.Move(partial, destination, overwrite: true);
        progress?.Report(new GgufDownloadProgress
        {
            BytesReceived = received,
            TotalBytes = received,
            Status = "Downloaded"
        });
    }

    public void Delete(GgufArtifact artifact, string? storageDir)
    {
        var destination = GgufStorage.DestinationPath(artifact, storageDir);
        TryDelete(destination);
        TryDelete(GgufStorage.PartialPath(destination));
        TryDelete(GgufStorage.MetaPath(destination));
    }

    public void Dispose()
    {
        if (_disposeHandler)
        {
            _handler.Dispose();
        }
    }

    public static string FormatSize(long bytes)
    {
        if (bytes < 1024)
        {
            return $"{bytes} B";
        }

        if (bytes < 1024 * 1024)
        {
            return $"{bytes / 1024.0:0.0} KB";
        }

        if (bytes < 1024L * 1024 * 1024)
        {
            return $"{bytes / (1024.0 * 1024):0.0} MB";
        }

        return $"{bytes / (1024.0 * 1024 * 1024):0.00} GB";
    }

    private static long FileSize(string path) => File.Exists(path) ? new FileInfo(path).Length : 0;

    private static void TryDelete(string path)
    {
        if (File.Exists(path))
        {
            File.Delete(path);
        }
    }

    private static GgufDownloadMeta? ReadMeta(string destination)
    {
        var metaPath = GgufStorage.MetaPath(destination);
        if (!File.Exists(metaPath))
        {
            return null;
        }

        try
        {
            return AppJson.Deserialize<GgufDownloadMeta>(File.ReadAllText(metaPath));
        }
        catch (System.Text.Json.JsonException)
        {
            return null;
        }
    }

    private static void WriteMeta(string destination, GgufDownloadMeta meta)
    {
        File.WriteAllText(GgufStorage.MetaPath(destination), AppJson.Serialize(meta));
    }
}
