using SQLitePCL;

namespace AiFileSorter.Core.Persistence;

/// <summary>Thin SQLitePCLRaw wrapper that avoids Microsoft.Data.Sqlite reflection (IL2113 on AOT).</summary>
public sealed class NativeSqlite : IDisposable
{
    private sqlite3? _db;
    private bool _disposed;

    static NativeSqlite()
    {
        Batteries_V2.Init();
    }

    public NativeSqlite(string path)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(path);
        var directory = Path.GetDirectoryName(path);
        if (!string.IsNullOrWhiteSpace(directory))
        {
            Directory.CreateDirectory(directory);
        }

        var flags = raw.SQLITE_OPEN_READWRITE | raw.SQLITE_OPEN_CREATE | raw.SQLITE_OPEN_FULLMUTEX;
        var rc = raw.sqlite3_open_v2(path, out _db, flags, null);
        if (rc != raw.SQLITE_OK)
        {
            var message = _db is null ? $"sqlite open failed ({rc})" : raw.sqlite3_errmsg(_db).utf8_to_string();
            _db?.Dispose();
            _db = null;
            throw new InvalidOperationException(message);
        }

        Execute("PRAGMA journal_mode=WAL;");
        Execute("PRAGMA busy_timeout=5000;");
        Execute("PRAGMA foreign_keys=ON;");
        Execute("PRAGMA synchronous=NORMAL;");
    }

    public static void EnsureInitialized() => Batteries_V2.Init();

    public void Execute(string sql)
    {
        EnsureOpen();
        var rc = raw.sqlite3_exec(_db, sql);
        if (rc != raw.SQLITE_OK)
        {
            throw new InvalidOperationException(Error("exec"));
        }
    }

    public NativeSqliteStatement Prepare(string sql)
    {
        EnsureOpen();
        var rc = raw.sqlite3_prepare_v2(_db, sql, out var stmt);
        if (rc != raw.SQLITE_OK || stmt is null)
        {
            stmt?.Dispose();
            throw new InvalidOperationException(Error("prepare"));
        }

        return new NativeSqliteStatement(stmt, Error);
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }

        _disposed = true;
        _db?.Dispose();
        _db = null;
    }

    private void EnsureOpen()
    {
        ObjectDisposedException.ThrowIf(_disposed || _db is null, this);
    }

    private string Error(string action)
    {
        var message = _db is null ? "database is closed" : raw.sqlite3_errmsg(_db).utf8_to_string();
        return $"sqlite {action} failed: {message}";
    }
}

public sealed class NativeSqliteStatement : IDisposable
{
    private sqlite3_stmt? _stmt;
    private readonly Func<string, string> _error;
    private bool _disposed;

    internal NativeSqliteStatement(sqlite3_stmt stmt, Func<string, string> error)
    {
        _stmt = stmt;
        _error = error;
    }

    public void Bind(int index, string? value)
    {
        EnsureOpen();
        var rc = value is null
            ? raw.sqlite3_bind_null(_stmt, index)
            : raw.sqlite3_bind_text(_stmt, index, value);
        if (rc != raw.SQLITE_OK)
        {
            throw new InvalidOperationException(_error("bind"));
        }
    }

    public void Bind(int index, int value)
    {
        EnsureOpen();
        var rc = raw.sqlite3_bind_int(_stmt, index, value);
        if (rc != raw.SQLITE_OK)
        {
            throw new InvalidOperationException(_error("bind"));
        }
    }

    public bool StepRow()
    {
        EnsureOpen();
        var rc = raw.sqlite3_step(_stmt);
        if (rc == raw.SQLITE_ROW)
        {
            return true;
        }

        if (rc == raw.SQLITE_DONE)
        {
            return false;
        }

        throw new InvalidOperationException(_error("step"));
    }

    public void StepDone()
    {
        EnsureOpen();
        var rc = raw.sqlite3_step(_stmt);
        if (rc != raw.SQLITE_DONE)
        {
            throw new InvalidOperationException(_error("step"));
        }
    }

    public string? Text(int column)
    {
        EnsureOpen();
        if (raw.sqlite3_column_type(_stmt, column) == raw.SQLITE_NULL)
        {
            return null;
        }

        return raw.sqlite3_column_text(_stmt, column).utf8_to_string();
    }

    public int Int(int column)
    {
        EnsureOpen();
        return raw.sqlite3_column_int(_stmt, column);
    }

    public void Reset()
    {
        EnsureOpen();
        raw.sqlite3_reset(_stmt);
        raw.sqlite3_clear_bindings(_stmt);
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }

        _disposed = true;
        if (_stmt is not null)
        {
            raw.sqlite3_finalize(_stmt);
            _stmt = null;
        }
    }

    private void EnsureOpen()
    {
        ObjectDisposedException.ThrowIf(_disposed || _stmt is null, this);
    }
}
