using AiFileSorter.Core.Json;
using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Persistence;

/// <summary>SQLite suggestion database with WAL, per-file rows, and remote revision history.</summary>
public sealed class SuggestionDatabase : IDisposable
{
    private readonly NativeSqlite _db;
    private readonly object _gate = new();

    public SuggestionDatabase(string databasePath)
    {
        _db = new NativeSqlite(databasePath);
        Initialize();
    }

    public void SavePlan(FilingPlan plan)
    {
        lock (_gate)
        {
            _db.Execute("BEGIN IMMEDIATE;");
            try
            {
                using (var deleteSuggestions = _db.Prepare("DELETE FROM suggestions WHERE plan_id = ?;"))
                {
                    deleteSuggestions.Bind(1, plan.PlanId);
                    deleteSuggestions.StepDone();
                }

                using (var upsertPlan = _db.Prepare("""
                    INSERT INTO plans(plan_id, root_path, created_at, json)
                    VALUES(?, ?, ?, ?)
                    ON CONFLICT(plan_id) DO UPDATE SET
                        root_path = excluded.root_path,
                        created_at = excluded.created_at,
                        json = excluded.json;
                    """))
                {
                    upsertPlan.Bind(1, plan.PlanId);
                    upsertPlan.Bind(2, plan.RootPath);
                    upsertPlan.Bind(3, plan.CreatedAtUtc.ToString("O"));
                    upsertPlan.Bind(4, AppJson.Serialize(plan));
                    upsertPlan.StepDone();
                }

                using var insertSuggestion = _db.Prepare("""
                    INSERT INTO suggestions(
                        plan_id, full_path, file_name, kind, family, category, subcategory,
                        local_relative_path, remote_relative_path, accepted_relative_path,
                        suggested_name, rationale, status, selected)
                    VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);
                    """);
                foreach (var entry in plan.Entries)
                {
                    insertSuggestion.Reset();
                    insertSuggestion.Bind(1, plan.PlanId);
                    insertSuggestion.Bind(2, entry.FullPath);
                    insertSuggestion.Bind(3, entry.FileName);
                    insertSuggestion.Bind(4, entry.Kind.ToString());
                    insertSuggestion.Bind(5, entry.Family.ToString());
                    insertSuggestion.Bind(6, entry.Category);
                    insertSuggestion.Bind(7, entry.Subcategory);
                    insertSuggestion.Bind(8, entry.LocalRelativePath);
                    insertSuggestion.Bind(9, entry.RemoteRelativePath);
                    insertSuggestion.Bind(10, entry.AcceptedRelativePath);
                    insertSuggestion.Bind(11, entry.SuggestedName);
                    insertSuggestion.Bind(12, entry.Rationale);
                    insertSuggestion.Bind(13, entry.Status.ToString());
                    insertSuggestion.Bind(14, entry.Selected ? 1 : 0);
                    insertSuggestion.StepDone();
                }

                using (var insertRevision = _db.Prepare("""
                    INSERT INTO revisions(plan_id, created_at, source, json)
                    VALUES(?, ?, ?, ?);
                    """))
                {
                    insertRevision.Bind(1, plan.PlanId);
                    insertRevision.Bind(2, DateTimeOffset.UtcNow.ToString("O"));
                    insertRevision.Bind(3, "local");
                    insertRevision.Bind(4, AppJson.Serialize(plan));
                    insertRevision.StepDone();
                }

                _db.Execute("COMMIT;");
            }
            catch
            {
                _db.Execute("ROLLBACK;");
                throw;
            }
        }
    }

    public void SaveRemoteRevision(FilingPlan plan, RemoteStructureSuggestion proposal)
    {
        lock (_gate)
        {
            SavePlan(plan);
            using var insertRevision = _db.Prepare("""
                INSERT INTO revisions(plan_id, created_at, source, json)
                VALUES(?, ?, ?, ?);
                """);
            insertRevision.Bind(1, plan.PlanId);
            insertRevision.Bind(2, DateTimeOffset.UtcNow.ToString("O"));
            insertRevision.Bind(3, "remote");
            insertRevision.Bind(4, AppJson.Serialize(proposal));
            insertRevision.StepDone();
        }
    }

    public FilingPlan? LoadLatestPlan(string rootPath)
    {
        var full = Path.GetFullPath(rootPath);
        lock (_gate)
        {
            using var stmt = _db.Prepare("""
                SELECT json FROM plans
                WHERE root_path = ?
                ORDER BY created_at DESC
                LIMIT 1;
                """);
            stmt.Bind(1, full);
            if (!stmt.StepRow())
            {
                return null;
            }

            return DeserializePlan(stmt.Text(0));
        }
    }

    public FilingPlan? LoadPlan(string planId)
    {
        lock (_gate)
        {
            using var stmt = _db.Prepare("SELECT json FROM plans WHERE plan_id = ?;");
            stmt.Bind(1, planId);
            if (!stmt.StepRow())
            {
                return null;
            }

            return DeserializePlan(stmt.Text(0));
        }
    }

    public IReadOnlyList<string> ListPlanIds()
    {
        lock (_gate)
        {
            using var stmt = _db.Prepare("SELECT plan_id FROM plans ORDER BY created_at DESC;");
            var ids = new List<string>();
            while (stmt.StepRow())
            {
                var id = stmt.Text(0);
                if (!string.IsNullOrWhiteSpace(id))
                {
                    ids.Add(id);
                }
            }

            return ids;
        }
    }

    public void Dispose() => _db.Dispose();

    private void Initialize()
    {
        _db.Execute("""
            CREATE TABLE IF NOT EXISTS plans (
                plan_id TEXT PRIMARY KEY,
                root_path TEXT NOT NULL,
                created_at TEXT NOT NULL,
                json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS suggestions (
                id INTEGER PRIMARY KEY,
                plan_id TEXT NOT NULL,
                full_path TEXT NOT NULL,
                file_name TEXT NOT NULL,
                kind TEXT NOT NULL,
                family TEXT NOT NULL,
                category TEXT,
                subcategory TEXT,
                local_relative_path TEXT,
                remote_relative_path TEXT,
                accepted_relative_path TEXT,
                suggested_name TEXT,
                rationale TEXT,
                status TEXT NOT NULL,
                selected INTEGER NOT NULL DEFAULT 1,
                UNIQUE(plan_id, full_path)
            );
            CREATE TABLE IF NOT EXISTS revisions (
                id INTEGER PRIMARY KEY,
                plan_id TEXT NOT NULL,
                created_at TEXT NOT NULL,
                source TEXT NOT NULL,
                json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_plans_root ON plans(root_path, created_at);
            CREATE INDEX IF NOT EXISTS idx_suggestions_plan ON suggestions(plan_id);
            """);
    }

    private static FilingPlan? DeserializePlan(string? json)
    {
        if (string.IsNullOrWhiteSpace(json))
        {
            return null;
        }

        try
        {
            return AppJson.Deserialize<FilingPlan>(json);
        }
        catch (System.Text.Json.JsonException)
        {
            return null;
        }
    }
}
