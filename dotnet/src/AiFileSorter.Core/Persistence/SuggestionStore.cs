using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Persistence;

/// <summary>Persists filing plans to SQLite by default, with JSON fallback for .json paths.</summary>
public sealed class SuggestionStore : IDisposable
{
    private readonly JsonPlanCatalog? _json;
    private readonly SuggestionDatabase? _sqlite;

    public SuggestionStore(string databasePath)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(databasePath);
        if (IsJsonPath(databasePath))
        {
            _json = new JsonPlanCatalog(databasePath);
        }
        else
        {
            _sqlite = new SuggestionDatabase(databasePath);
        }
    }

    public void SavePlan(FilingPlan plan)
    {
        if (_sqlite is not null)
        {
            _sqlite.SavePlan(plan);
            return;
        }

        _json!.SavePlan(plan);
    }

    public void SaveRemoteRevision(FilingPlan plan, RemoteStructureSuggestion proposal)
    {
        if (_sqlite is not null)
        {
            _sqlite.SaveRemoteRevision(plan, proposal);
            return;
        }

        _json!.SavePlan(plan);
    }

    public FilingPlan? LoadLatestPlan(string rootPath) =>
        _sqlite?.LoadLatestPlan(rootPath) ?? _json!.LoadLatestPlan(rootPath);

    public FilingPlan? LoadPlan(string planId) =>
        _sqlite?.LoadPlan(planId) ?? _json!.LoadPlan(planId);

    public IReadOnlyList<string> ListPlanIds() =>
        _sqlite?.ListPlanIds() ?? _json!.ListPlanIds();

    public void Dispose()
    {
        _sqlite?.Dispose();
        _json?.Dispose();
    }

    public static bool IsJsonPath(string path) =>
        path.EndsWith(".json", StringComparison.OrdinalIgnoreCase);
}

public sealed class JsonPlanCatalog : IDisposable
{
    private readonly string _path;
    private readonly object _gate = new();

    public JsonPlanCatalog(string databasePath)
    {
        _path = databasePath;
        var directory = Path.GetDirectoryName(databasePath);
        if (!string.IsNullOrWhiteSpace(directory))
        {
            Directory.CreateDirectory(directory);
        }
    }

    public void SavePlan(FilingPlan plan)
    {
        lock (_gate)
        {
            var catalog = ReadCatalog();
            var plans = catalog.Plans
                .Where(existing => existing.PlanId != plan.PlanId)
                .Prepend(plan)
                .Take(25)
                .ToArray();
            var json = Json.AppJson.Serialize(new PlanCatalog { Plans = plans });
            var temp = _path + ".tmp";
            File.WriteAllText(temp, json);
            File.Move(temp, _path, overwrite: true);
        }
    }

    public FilingPlan? LoadLatestPlan(string rootPath)
    {
        var full = Path.GetFullPath(rootPath);
        return ReadCatalog().Plans.FirstOrDefault(plan =>
            string.Equals(plan.RootPath, full, StringComparison.OrdinalIgnoreCase));
    }

    public FilingPlan? LoadPlan(string planId) =>
        ReadCatalog().Plans.FirstOrDefault(plan => plan.PlanId == planId);

    public IReadOnlyList<string> ListPlanIds() =>
        ReadCatalog().Plans.Select(plan => plan.PlanId).ToArray();

    public void Dispose()
    {
    }

    private PlanCatalog ReadCatalog()
    {
        if (!File.Exists(_path))
        {
            return new PlanCatalog();
        }

        try
        {
            return Json.AppJson.Deserialize<PlanCatalog>(File.ReadAllText(_path));
        }
        catch (System.Text.Json.JsonException)
        {
            return new PlanCatalog();
        }
    }
}

public sealed record PlanCatalog
{
    public IReadOnlyList<FilingPlan> Plans { get; init; } = [];
}
