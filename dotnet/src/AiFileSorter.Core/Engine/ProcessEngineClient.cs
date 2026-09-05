using System.Diagnostics;
using System.Text;
using AiFileSorter.Core.Ipc;
using AiFileSorter.Core.Json;
using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Engine;

/// <summary>Talks to a child `aifs engine` process so file locks and crashes cannot freeze the UI.</summary>
public sealed class ProcessEngineClient : IAsyncDisposable
{
    private readonly Process _process;
    private readonly SemaphoreSlim _writeGate = new(1, 1);

    private ProcessEngineClient(Process process)
    {
        _process = process;
    }

    public static string? FindExecutable()
    {
        var directory = AppContext.BaseDirectory;
        foreach (var name in new[] { "aifs", "aifs.exe" })
        {
            var candidate = Path.Combine(directory, name);
            if (File.Exists(candidate))
            {
                return candidate;
            }
        }

        return null;
    }

    public static ProcessEngineClient? TryStart()
    {
        var executable = FindExecutable();
        if (executable is null)
        {
            return null;
        }

        var start = new ProcessStartInfo
        {
            FileName = executable,
            Arguments = "engine",
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true
        };
        start.StandardOutputEncoding = Encoding.UTF8;
        start.StandardInputEncoding = Encoding.UTF8;

        var process = Process.Start(start);
        return process is null ? null : new ProcessEngineClient(process);
    }

    public async Task<FilingPlan> AnalyzeAsync(
        AnalysisRequest request,
        IProgress<AnalysisProgress>? progress,
        CancellationToken cancellationToken)
    {
        var id = Guid.NewGuid().ToString("N");
        var command = new EngineCommand
        {
            Op = "analyze",
            Id = id,
            Request = request
        };

        await _writeGate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            await _process.StandardInput.WriteLineAsync(AppJson.Serialize(command)).ConfigureAwait(false);
            await _process.StandardInput.FlushAsync().ConfigureAwait(false);

            while (true)
            {
                cancellationToken.ThrowIfCancellationRequested();
                var line = await _process.StandardOutput.ReadLineAsync(cancellationToken).ConfigureAwait(false);
                if (line is null)
                {
                    throw new InvalidOperationException("Engine process closed unexpectedly.");
                }

                var engineEvent = AppJson.Deserialize<EngineEvent>(line);
                if (!string.Equals(engineEvent.Id, id, StringComparison.Ordinal) &&
                    !string.IsNullOrWhiteSpace(engineEvent.Id))
                {
                    continue;
                }

                if (engineEvent.Type == "progress" && engineEvent.Progress is not null)
                {
                    progress?.Report(engineEvent.Progress);
                    continue;
                }

                if (engineEvent.Type == "completed" && engineEvent.Plan is not null)
                {
                    return engineEvent.Plan;
                }

                if (engineEvent.Type == "failed")
                {
                    throw new InvalidOperationException(engineEvent.Error ?? "Engine failed.");
                }
            }
        }
        finally
        {
            _writeGate.Release();
        }
    }

    public async ValueTask DisposeAsync()
    {
        try
        {
            if (!_process.HasExited)
            {
                _process.Kill(entireProcessTree: true);
            }
        }
        catch (InvalidOperationException)
        {
        }

        _process.Dispose();
        _writeGate.Dispose();
        await Task.CompletedTask.ConfigureAwait(false);
    }
}
