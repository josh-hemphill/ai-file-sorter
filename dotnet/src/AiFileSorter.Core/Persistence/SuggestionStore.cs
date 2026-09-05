using AiFileSorter.Core.Json;
using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Persistence;

public sealed record PlanCatalog
{
    public IReadOnlyList<FilingPlan> Plans { get; init; } = [];
}

/// <summary>Persists filing plans as JSON so Native AOT UI/CLI builds do not need SQLite reflection.</summary>
public sealed class SuggestionStore : IDisposable
{
    private readonly string _path;
    private readonly object _gate = new();

    public SuggestionStore(string databasePath)
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
            var json = AppJson.Serialize(new PlanCatalog { Plans = plans });
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
            return AppJson.Deserialize<PlanCatalog>(File.ReadAllText(_path));
        }
        catch (System.Text.Json.JsonException)
        {
            return new PlanCatalog();
        }
    }
}
