using AiFileSorter.Core.Engine;
using AiFileSorter.Core.Ipc;
using AiFileSorter.Core.Json;
using AiFileSorter.Core.Models;
using AiFileSorter.Core.Persistence;
using AiFileSorter.Core.Plans;

namespace AiFileSorter.Cli;

public static class Program
{
    public static async Task<int> Main(string[] args)
    {
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
            DatabasePath = options.DatabasePath ?? AnalysisEngine.DefaultDatabasePath()
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
        Console.WriteLine(AppJson.Serialize(suggestion));
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

    private static void PrintHelp()
    {
        Console.WriteLine("""
            AI File Sorter Avalonia engine
            Usage:
              aifs analyze <folder> [--recursive] [--consistent] [--db <path>]
              aifs handoff <folder> [--prompt-only] [--plan-id <id>] [--endpoint <url>] [--model <name>] [--api-key <key>]
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
}
