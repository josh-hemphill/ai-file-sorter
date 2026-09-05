using System.Diagnostics;
using System.Text;
using AiFileSorter.Core.Json;

namespace AiFileSorter.Core.Llm;

public sealed record LlamaCompleteRequest
{
    public string ModelPath { get; init; } = "";
    public string? MmprojPath { get; init; }
    public string SystemPrompt { get; init; } = "";
    public string UserPrompt { get; init; } = "";
    public int MaxTokens { get; init; } = 96;
}

public sealed record LlamaCompleteResponse
{
    public string Text { get; init; } = "";
    public string? Error { get; init; }
}

public interface ILocalLlmClient
{
    bool IsAvailable { get; }
    string Source { get; }
    string Complete(LlamaCompleteRequest request, CancellationToken cancellationToken = default);
}

/// <summary>Runs GGUF inference in a child `aifs-llama` process so llama.cpp never lives in the AOT UI.</summary>
public sealed class ProcessLlamaClient : ILocalLlmClient, IDisposable
{
    private readonly string _executable;

    public ProcessLlamaClient(string executable)
    {
        _executable = executable;
    }

    public bool IsAvailable => File.Exists(_executable);
    public string Source => _executable;

    public static string? FindExecutable()
    {
        var directory = AppContext.BaseDirectory;
        foreach (var name in new[] { "aifs-llama", "aifs-llama.exe" })
        {
            var candidate = Path.Combine(directory, name);
            if (File.Exists(candidate))
            {
                return candidate;
            }
        }

        var path = Environment.GetEnvironmentVariable("PATH") ?? "";
        foreach (var folder in path.Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries))
        {
            foreach (var name in new[] { "aifs-llama", "aifs-llama.exe" })
            {
                var candidate = Path.Combine(folder, name);
                if (File.Exists(candidate))
                {
                    return candidate;
                }
            }
        }

        return null;
    }

    public static ProcessLlamaClient? TryCreate()
    {
        var executable = FindExecutable();
        return executable is null ? null : new ProcessLlamaClient(executable);
    }

    public string Complete(LlamaCompleteRequest request, CancellationToken cancellationToken = default)
    {
        if (!File.Exists(request.ModelPath))
        {
            throw new FileNotFoundException("Local GGUF model is not downloaded.", request.ModelPath);
        }

        var start = new ProcessStartInfo
        {
            FileName = _executable,
            Arguments = "complete",
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true
        };
        start.StandardInputEncoding = Encoding.UTF8;
        start.StandardOutputEncoding = Encoding.UTF8;

        using var process = Process.Start(start) ?? throw new InvalidOperationException("Failed to start aifs-llama.");
        process.StandardInput.WriteLine(AppJson.Serialize(request));
        process.StandardInput.Close();

        var stdout = process.StandardOutput.ReadToEnd();
        var stderr = process.StandardError.ReadToEnd();
        if (!process.WaitForExit(120_000))
        {
            try
            {
                process.Kill(entireProcessTree: true);
            }
            catch (InvalidOperationException)
            {
            }

            throw new TimeoutException("aifs-llama timed out.");
        }

        if (process.ExitCode != 0)
        {
            throw new InvalidOperationException(string.IsNullOrWhiteSpace(stderr) ? stdout : stderr);
        }

        var parsed = AppJson.Deserialize<LlamaCompleteResponse>(stdout);
        if (!string.IsNullOrWhiteSpace(parsed.Error))
        {
            throw new InvalidOperationException(parsed.Error);
        }

        return parsed.Text;
    }

    public void Dispose()
    {
    }
}

public static class LocalLlmFactory
{
    public static ILocalLlmClient? TryCreate(ILocalLlmClient? overrideClient = null) =>
        overrideClient ?? ProcessLlamaClient.TryCreate();
}
