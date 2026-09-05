using AiFileSorter.Core.Llm;
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
    private readonly ApplyService _applyService;

    public AnalysisEngine()
        : this(new FileScanner(), new FilingPlanBuilder(), new RemotePlanHandoff(), new ApplyService())
    {
    }

    public AnalysisEngine(
        FileScanner scanner,
        FilingPlanBuilder planBuilder,
        RemotePlanHandoff handoff,
        ApplyService applyService)
    {
        _scanner = scanner;
        _planBuilder = planBuilder;
        _handoff = handoff;
        _applyService = applyService;
    }

    public async Task<FilingPlan> AnalyzeAsync(
        AnalysisRequest request,
        IProgress<AnalysisProgress>? progress,
        CancellationToken cancellationToken = default)
    {
        NativeSqlite.EnsureInitialized();
        var scanOptions = request.Scan.WithContent(request.Content);
        progress?.Report(new AnalysisProgress { Stage = "scan", Message = "Scanning folder…" });
        var scan = await Task.Run(
                () => _scanner.Scan(request.RootPath, scanOptions, cancellationToken),
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

    public FilingPlan MergeRemote(FilingPlan plan, RemoteStructureSuggestion proposal, string? databasePath)
    {
        var merged = RemoteProposalMerger.Merge(plan, proposal);
        if (!string.IsNullOrWhiteSpace(databasePath))
        {
            using var store = new SuggestionStore(databasePath);
            store.SaveRemoteRevision(merged, proposal);
        }

        return merged;
    }

    public ApplyDryRun PreviewApply(FilingPlan plan) => _applyService.Preview(plan);

    public ApplyResult Apply(FilingPlan plan, bool dryRun, string? databasePath)
    {
        var result = _applyService.Apply(plan, dryRun);
        if (!dryRun && result.Applied.Count > 0)
        {
            var updated = ApplyService.MarkApplied(plan, result);
            if (!string.IsNullOrWhiteSpace(databasePath))
            {
                using var store = new SuggestionStore(databasePath);
                store.SavePlan(updated);
            }
        }

        return result;
    }

    public static async Task<RemoteStructureSuggestion> RequestRemoteProposalAsync(
        FilingPlan plan,
        LlmEndpointSettings settings,
        CancellationToken cancellationToken = default)
    {
        var resolved = LlmCatalog.ResolveRemoteEndpoint(settings);
        if (resolved is null)
        {
            throw new InvalidOperationException("Select OpenAI, Gemini, or a custom API and provide credentials before asking a remote model.");
        }

        var payload = new RemotePlanHandoff().Create(plan);
        using var http = new HttpClient { Timeout = TimeSpan.FromSeconds(60) };
        var client = new OpenAiCompatiblePlanClient(http, resolved.Value.EndpointUrl, resolved.Value.Model, resolved.Value.ApiKey);
        return await client.SuggestStructureAsync(payload, cancellationToken).ConfigureAwait(false);
    }

    public static string DefaultDatabasePath()
    {
        var root = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
        if (string.IsNullOrWhiteSpace(root))
        {
            root = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".local", "share");
        }

        return Path.Combine(root, "AIFileSorter", "avalonia", "suggestions.sqlite");
    }
}
