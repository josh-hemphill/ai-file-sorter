using AiFileSorter.Core.Models;
using AiFileSorter.Core.Persistence;
using AiFileSorter.Core.Plans;
using AiFileSorter.Core.Scanning;

namespace AiFileSorter.Core.Engine;

/// <summary>Runs scan, categorization, and persistence off the UI thread.</summary>
public sealed class AnalysisEngine
{
    private readonly FileScanner _scanner;
    private readonly FilingPlanBuilder _planBuilder;
    private readonly RemotePlanHandoff _handoff;

    public AnalysisEngine()
        : this(new FileScanner(), new FilingPlanBuilder(), new RemotePlanHandoff())
    {
    }

    public AnalysisEngine(FileScanner scanner, FilingPlanBuilder planBuilder, RemotePlanHandoff handoff)
    {
        _scanner = scanner;
        _planBuilder = planBuilder;
        _handoff = handoff;
    }

    public async Task<FilingPlan> AnalyzeAsync(
        AnalysisRequest request,
        IProgress<AnalysisProgress>? progress,
        CancellationToken cancellationToken = default)
    {
        progress?.Report(new AnalysisProgress { Stage = "scan", Message = "Scanning folder…" });
        var scan = await Task.Run(
                () => _scanner.Scan(request.RootPath, request.Scan, cancellationToken),
                cancellationToken)
            .ConfigureAwait(false);

        progress?.Report(new AnalysisProgress
        {
            Stage = "categorize",
            Current = 0,
            Total = scan.Items.Count,
            Message = "Categorizing…"
        });

        var plan = await Task.Run(
                () => _planBuilder.Build(request.RootPath, scan, request, progress, cancellationToken),
                cancellationToken)
            .ConfigureAwait(false);

        if (!string.IsNullOrWhiteSpace(request.DatabasePath))
        {
            progress?.Report(new AnalysisProgress { Stage = "persist", Message = "Saving plan…" });
            await Task.Run(
                    () =>
                    {
                        using var store = new SuggestionStore(request.DatabasePath);
                        store.SavePlan(plan);
                    },
                    cancellationToken)
                .ConfigureAwait(false);
        }

        progress?.Report(new AnalysisProgress
        {
            Stage = "done",
            Current = plan.Entries.Count,
            Total = plan.Entries.Count,
            Message = "Analysis complete."
        });
        return plan;
    }

    public RemoteHandoffPayload CreateHandoff(FilingPlan plan) => _handoff.Create(plan);

    public static string DefaultDatabasePath()
    {
        var root = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
        if (string.IsNullOrWhiteSpace(root))
        {
            root = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".local", "share");
        }

        return Path.Combine(root, "AIFileSorter", "avalonia", "suggestions.json");
    }
}
