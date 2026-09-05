using AiFileSorter.Core.Engine;
using AiFileSorter.Core.Ipc;
using AiFileSorter.Core.Json;
using AiFileSorter.Core.Llm;
using AiFileSorter.Core.Models;
using AiFileSorter.Core.Persistence;
using AiFileSorter.Core.Plans;

namespace AiFileSorter.Cli;

public static class Program
{
    public static async Task<int> Main(string[] args)
    {
        NativeSqlite.EnsureInitialized();
        if (args.Length == 0 || args[0] is "-h" or "--help" or "help")
        {
            PrintHelp();
            return 0;
        }

        try
        {
            return args[0] switch
            {
                "analyze" => await AnalyzeAsync(args[1..]).ConfigureAwait(false),
                "handoff" => await HandoffAsync(args[1..]).ConfigureAwait(false),
                "merge" => await MergeAsync(args[1..]).ConfigureAwait(false),
                "apply" => await ApplyAsync(args[1..]).ConfigureAwait(false),
                "models" => ListModels(),
                "download" => await DownloadModelAsync(args[1..]).ConfigureAwait(false),
                "engine" => await RunEngineAsync().ConfigureAwait(false),
                _ => Fail($"Unknown command '{args[0]}'.")
            };
        }
        catch (Exception ex)
        {
            await Console.Error.WriteLineAsync(ex.Message).ConfigureAwait(false);
            return 1;
        }
    }

    private static async Task<int> AnalyzeAsync(string[] args)
    {
        var options = ParseAnalyzeOptions(args);
        if (string.IsNullOrWhiteSpace(options.Path))
        {
            return Fail("analyze requires a folder path.");
        }

        var engine = new AnalysisEngine();
        var request = new AnalysisRequest
        {
            RootPath = options.Path,
            Scan = new ScanOptions
            {
                Flags = ScanFlags.Files | ScanFlags.Directories |
                        (options.Recursive ? ScanFlags.Recursive : ScanFlags.None)
            },
            Style = options.Consistent ? CategorizationStyle.Consistent : CategorizationStyle.Refined,
            DatabasePath = options.DatabasePath ?? AnalysisEngine.DefaultDatabasePath(),
            Content = new ContentAnalysisOptions
            {
                IncludeSubdirectories = options.Recursive,
                UseSubcategories = options.UseSubcategories,
                AnalyzeMedia = options.AnalyzeMedia,
                OfferRenameMedia = options.AnalyzeMedia,
                ProcessDocumentsOnly = options.DocumentsOnly,
                ProcessImagesOnly = options.ImagesOnly
            }
        };

        var progress = new Progress<AnalysisProgress>(update =>
        {
            Console.Error.WriteLine($"{update.Stage}: {update.Message}");
        });
        var plan = await engine.AnalyzeAsync(request, progress).ConfigureAwait(false);
        Console.WriteLine(AppJson.Serialize(plan));
        return 0;
    }

    private static async Task<int> HandoffAsync(string[] args)
    {
        var options = ParseHandoffOptions(args);
        using var store = new SuggestionStore(options.DatabasePath ?? AnalysisEngine.DefaultDatabasePath());
        var plan = options.PlanId is null
            ? store.LoadLatestPlan(options.Path ?? throw new InvalidOperationException("handoff requires --path or --plan-id."))
            : store.LoadPlan(options.PlanId);
        if (plan is null)
        {
            return Fail("No saved filing plan was found.");
        }

        var payload = new RemotePlanHandoff().Create(plan);
        if (options.PromptOnly || string.IsNullOrWhiteSpace(options.Endpoint))
        {
            Console.WriteLine(options.PromptOnly ? payload.CompactPrompt : AppJson.Serialize(payload));
            return 0;
        }

        using var http = new HttpClient { Timeout = TimeSpan.FromSeconds(60) };
        var client = new OpenAiCompatiblePlanClient(http, options.Endpoint, options.Model, options.ApiKey);
        var suggestion = await client.SuggestStructureAsync(payload).ConfigureAwait(false);
        var dbPath = options.DatabasePath ?? AnalysisEngine.DefaultDatabasePath();
        var merged = new AnalysisEngine().MergeRemote(plan, suggestion, dbPath);
        var remoteCount = merged.Entries.Count(entry => entry.Status == SuggestionStatus.RemoteProposed);
        Console.Error.WriteLine($"Merged {remoteCount} remote path proposals into {dbPath}. Files were not moved.");
        Console.WriteLine(AppJson.Serialize(suggestion));
        return 0;
    }

    private static Task<int> MergeAsync(string[] args)
    {
        var options = ParseMergeOptions(args);
        if (string.IsNullOrWhiteSpace(options.ProposalPath) || !File.Exists(options.ProposalPath))
        {
            return Task.FromResult(Fail("merge requires a proposal JSON file."));
        }

        if (string.IsNullOrWhiteSpace(options.Path))
        {
            return Task.FromResult(Fail("merge requires --path."));
        }

        var proposal = AppJson.Deserialize<RemoteStructureSuggestion>(File.ReadAllText(options.ProposalPath));
        var dbPath = options.DatabasePath ?? AnalysisEngine.DefaultDatabasePath();
        using var store = new SuggestionStore(dbPath);
        var plan = options.PlanId is null
            ? store.LoadLatestPlan(options.Path)
            : store.LoadPlan(options.PlanId);
        if (plan is null)
        {
            return Task.FromResult(Fail("No saved filing plan was found."));
        }

        var merged = new AnalysisEngine().MergeRemote(plan, proposal, dbPath);
        Console.WriteLine(AppJson.Serialize(merged));
        return Task.FromResult(0);
    }

    private static Task<int> ApplyAsync(string[] args)
    {
        var options = ParseApplyOptions(args);
        if (string.IsNullOrWhiteSpace(options.Path))
        {
            return Task.FromResult(Fail("apply requires a folder path."));
        }

        var dbPath = options.DatabasePath ?? AnalysisEngine.DefaultDatabasePath();
        using var store = new SuggestionStore(dbPath);
        var plan = options.PlanId is null
            ? store.LoadLatestPlan(options.Path)
            : store.LoadPlan(options.PlanId);
        if (plan is null)
        {
            return Task.FromResult(Fail("No saved filing plan was found."));
        }

        var result = new AnalysisEngine().Apply(plan, options.DryRun, dbPath);
        Console.WriteLine(AppJson.Serialize(result));
        return Task.FromResult(result.Errors.Count == 0 ? 0 : 1);
    }

    private static int ListModels()
    {
        foreach (var artifact in GgufCatalog.AllArtifacts)
        {
            Console.WriteLine($"{artifact.Id}\t{artifact.DisplayName}\t{GgufCatalog.FormatFunctions(artifact)}\t{artifact.ResolveUrl()}");
        }

        return 0;
    }

    private static async Task<int> DownloadModelAsync(string[] args)
    {
        if (args.Length == 0)
        {
            return Fail("download requires an artifact id. Use: aifs models");
        }

        var artifact = GgufCatalog.FindArtifact(args[0]);
        if (artifact is null)
        {
            return Fail($"Unknown artifact '{args[0]}'. Use: aifs models");
        }

        string? storage = null;
        for (var i = 1; i < args.Length; i++)
        {
            if (args[i] is "--dir" && i + 1 < args.Length)
            {
                storage = args[++i];
            }
        }

        using var downloader = new GgufDownloader();
        var progress = new Progress<GgufDownloadProgress>(update =>
        {
            Console.Error.WriteLine(update.Status);
        });
        await downloader.DownloadAsync(artifact, storage, progress).ConfigureAwait(false);
        var probe = downloader.Probe(artifact, storage);
        Console.WriteLine(probe.Path);
        return 0;
    }

    private static async Task<int> RunEngineAsync()
    {
        var engine = new AnalysisEngine();
        string? line;
        while ((line = await Console.In.ReadLineAsync().ConfigureAwait(false)) is not null)
        {
            if (string.IsNullOrWhiteSpace(line))
            {
                continue;
            }

            EngineCommand command;
            try
            {
                command = AppJson.Deserialize<EngineCommand>(line);
            }
            catch (Exception ex)
            {
                WriteEvent(new EngineEvent { Type = "failed", Error = "Invalid command: " + ex.Message });
                continue;
            }

            try
            {
                if (command.Op == "analyze")
                {
                    if (command.Request is null)
                    {
                        WriteEvent(new EngineEvent { Type = "failed", Id = command.Id, Error = "analyze requires request." });
                        continue;
                    }

                    var progress = new Progress<AnalysisProgress>(update =>
                    {
                        WriteEvent(new EngineEvent { Type = "progress", Id = command.Id, Progress = update });
                    });
                    var plan = await engine.AnalyzeAsync(command.Request, progress).ConfigureAwait(false);
                    WriteEvent(new EngineEvent { Type = "completed", Id = command.Id, Plan = plan });
                    continue;
                }

                if (command.Op == "handoff")
                {
                    if (command.Request?.RootPath is null && command.PlanId is null)
                    {
                        WriteEvent(new EngineEvent { Type = "failed", Id = command.Id, Error = "handoff requires planId or request.rootPath." });
                        continue;
                    }

                    using var store = new SuggestionStore(command.Request?.DatabasePath ?? AnalysisEngine.DefaultDatabasePath());
                    var plan = command.PlanId is not null
                        ? store.LoadPlan(command.PlanId)
                        : store.LoadLatestPlan(command.Request!.RootPath);
                    if (plan is null)
                    {
                        WriteEvent(new EngineEvent { Type = "failed", Id = command.Id, Error = "No saved plan." });
                        continue;
                    }

                    WriteEvent(new EngineEvent
                    {
                        Type = "handoff",
                        Id = command.Id,
                        Handoff = engine.CreateHandoff(plan)
                    });
                    continue;
                }

                WriteEvent(new EngineEvent { Type = "failed", Id = command.Id, Error = $"Unknown op '{command.Op}'." });
            }
            catch (Exception ex)
            {
                WriteEvent(new EngineEvent { Type = "failed", Id = command.Id, Error = ex.Message });
            }
        }

        return 0;
    }

    private static void WriteEvent(EngineEvent engineEvent)
    {
        Console.WriteLine(AppJson.Serialize(engineEvent));
    }

    private static AnalyzeOptions ParseAnalyzeOptions(string[] args)
    {
        var options = new AnalyzeOptions();
        for (var i = 0; i < args.Length; i++)
        {
            var arg = args[i];
            if (arg is "--recursive" or "-r")
            {
                options.Recursive = true;
            }
            else if (arg is "--consistent")
            {
                options.Consistent = true;
            }
            else if (arg.StartsWith("--db=", StringComparison.Ordinal))
            {
                options.DatabasePath = arg["--db=".Length..];
            }
            else if (arg is "--db" && i + 1 < args.Length)
            {
                options.DatabasePath = args[++i];
            }
            else if (arg is "--no-subcategories")
            {
                options.UseSubcategories = false;
            }
            else if (arg is "--no-media")
            {
                options.AnalyzeMedia = false;
            }
            else if (arg is "--documents-only")
            {
                options.DocumentsOnly = true;
            }
            else if (arg is "--images-only")
            {
                options.ImagesOnly = true;
            }
            else if (!arg.StartsWith('-'))
            {
                options.Path = arg;
            }
        }

        return options;
    }

    private static HandoffOptions ParseHandoffOptions(string[] args)
    {
        var options = new HandoffOptions();
        for (var i = 0; i < args.Length; i++)
        {
            var arg = args[i];
            if (arg is "--prompt-only")
            {
                options.PromptOnly = true;
            }
            else if (arg.StartsWith("--plan-id=", StringComparison.Ordinal))
            {
                options.PlanId = arg["--plan-id=".Length..];
            }
            else if (arg is "--plan-id" && i + 1 < args.Length)
            {
                options.PlanId = args[++i];
            }
            else if (arg.StartsWith("--endpoint=", StringComparison.Ordinal))
            {
                options.Endpoint = arg["--endpoint=".Length..];
            }
            else if (arg is "--endpoint" && i + 1 < args.Length)
            {
                options.Endpoint = args[++i];
            }
            else if (arg.StartsWith("--model=", StringComparison.Ordinal))
            {
                options.Model = arg["--model=".Length..];
            }
            else if (arg is "--model" && i + 1 < args.Length)
            {
                options.Model = args[++i];
            }
            else if (arg.StartsWith("--api-key=", StringComparison.Ordinal))
            {
                options.ApiKey = arg["--api-key=".Length..];
            }
            else if (arg is "--api-key" && i + 1 < args.Length)
            {
                options.ApiKey = args[++i];
            }
            else if (arg.StartsWith("--db=", StringComparison.Ordinal))
            {
                options.DatabasePath = arg["--db=".Length..];
            }
            else if (arg is "--db" && i + 1 < args.Length)
            {
                options.DatabasePath = args[++i];
            }
            else if (arg is "--path" && i + 1 < args.Length)
            {
                options.Path = args[++i];
            }
            else if (!arg.StartsWith('-'))
            {
                options.Path = arg;
            }
        }

        return options;
    }

    private static MergeOptions ParseMergeOptions(string[] args)
    {
        var options = new MergeOptions();
        for (var i = 0; i < args.Length; i++)
        {
            var arg = args[i];
            if (arg.StartsWith("--db=", StringComparison.Ordinal))
            {
                options.DatabasePath = arg["--db=".Length..];
            }
            else if (arg is "--db" && i + 1 < args.Length)
            {
                options.DatabasePath = args[++i];
            }
            else if (arg.StartsWith("--plan-id=", StringComparison.Ordinal))
            {
                options.PlanId = arg["--plan-id=".Length..];
            }
            else if (arg is "--plan-id" && i + 1 < args.Length)
            {
                options.PlanId = args[++i];
            }
            else if (arg is "--path" && i + 1 < args.Length)
            {
                options.Path = args[++i];
            }
            else if (!arg.StartsWith('-') && options.ProposalPath is null)
            {
                options.ProposalPath = arg;
            }
            else if (!arg.StartsWith('-'))
            {
                options.Path = arg;
            }
        }

        return options;
    }

    private static ApplyOptions ParseApplyOptions(string[] args)
    {
        var options = new ApplyOptions();
        for (var i = 0; i < args.Length; i++)
        {
            var arg = args[i];
            if (arg is "--dry-run")
            {
                options.DryRun = true;
            }
            else if (arg.StartsWith("--db=", StringComparison.Ordinal))
            {
                options.DatabasePath = arg["--db=".Length..];
            }
            else if (arg is "--db" && i + 1 < args.Length)
            {
                options.DatabasePath = args[++i];
            }
            else if (arg.StartsWith("--plan-id=", StringComparison.Ordinal))
            {
                options.PlanId = arg["--plan-id=".Length..];
            }
            else if (arg is "--plan-id" && i + 1 < args.Length)
            {
                options.PlanId = args[++i];
            }
            else if (!arg.StartsWith('-'))
            {
                options.Path = arg;
            }
        }

        return options;
    }

    private static void PrintHelp()
    {
        Console.WriteLine("""
            AI File Sorter Avalonia engine
            Usage:
              aifs analyze <folder> [--recursive] [--consistent] [--db <path>] [--no-subcategories] [--no-media] [--documents-only] [--images-only]
              aifs handoff <folder> [--prompt-only] [--plan-id <id>] [--endpoint <url>] [--model <name>] [--api-key <key>]
              aifs merge <proposal.json> --path <folder> [--db <path>]
              aifs apply <folder> [--dry-run] [--db <path>]
              aifs models
              aifs download <artifact-id> [--dir <storage>]
              aifs engine
            """);
    }

    private static int Fail(string message)
    {
        Console.Error.WriteLine(message);
        return 1;
    }

    private sealed class AnalyzeOptions
    {
        public string? Path { get; set; }
        public bool Recursive { get; set; } = true;
        public bool Consistent { get; set; }
        public string? DatabasePath { get; set; }
        public bool UseSubcategories { get; set; } = true;
        public bool AnalyzeMedia { get; set; } = true;
        public bool DocumentsOnly { get; set; }
        public bool ImagesOnly { get; set; }
    }

    private sealed class HandoffOptions
    {
        public string? Path { get; set; }
        public string? PlanId { get; set; }
        public bool PromptOnly { get; set; }
        public string? Endpoint { get; set; }
        public string Model { get; set; } = "gpt-4.1-mini";
        public string? ApiKey { get; set; }
        public string? DatabasePath { get; set; }
    }

    private sealed class MergeOptions
    {
        public string? Path { get; set; }
        public string? PlanId { get; set; }
        public string? ProposalPath { get; set; }
        public string? DatabasePath { get; set; }
    }

    private sealed class ApplyOptions
    {
        public string? Path { get; set; }
        public string? PlanId { get; set; }
        public bool DryRun { get; set; }
        public string? DatabasePath { get; set; }
    }
}
