namespace AiFileSorter.Core.Models;

[Flags]
public enum ScanFlags
{
    None = 0,
    Files = 1 << 0,
    Directories = 1 << 1,
    HiddenFiles = 1 << 2,
    Recursive = 1 << 3
}

public sealed record ScanOptions
{
    public ScanFlags Flags { get; init; } = ScanFlags.Files | ScanFlags.Directories;
    public bool ProtectProjectDirectories { get; init; } = true;
    public bool SuggestArchiveEntities { get; init; } = true;
    public bool SkipReparsePoints { get; init; } = true;
    public IReadOnlyList<string> AdditionalJunkNames { get; init; } = [];
    public IReadOnlyList<string> JunkNamePrefixes { get; init; } = [];

    public bool IncludeFiles => Flags.HasFlag(ScanFlags.Files);
    public bool IncludeDirectories => Flags.HasFlag(ScanFlags.Directories);
    public bool IncludeHidden => Flags.HasFlag(ScanFlags.HiddenFiles);
    public bool Recursive => Flags.HasFlag(ScanFlags.Recursive);

    public static ScanOptions DefaultRecursive() => new()
    {
        Flags = ScanFlags.Files | ScanFlags.Directories | ScanFlags.Recursive
    };

    public ScanOptions WithContent(ContentAnalysisOptions content)
    {
        var flags = ScanFlags.None;
        if (content.CategorizeFiles)
        {
            flags |= ScanFlags.Files;
        }

        if (content.CategorizeDirectories)
        {
            flags |= ScanFlags.Directories;
        }

        if (content.IncludeSubdirectories || Recursive)
        {
            flags |= ScanFlags.Recursive;
        }

        if (IncludeHidden)
        {
            flags |= ScanFlags.HiddenFiles;
        }

        return this with { Flags = flags };
    }
}

public enum EntryKind
{
    File,
    Directory
}

public enum FileFamily
{
    Generic,
    Image,
    Document,
    Software,
    Archive,
    Audio,
    Video,
    Ebook,
    Font
}

public enum ProjectStrength
{
    Weak,
    Strong
}

public enum ArchiveFormat
{
    Zip,
    TarGz
}

public enum CategorizationStyle
{
    Refined,
    Consistent
}

public enum SuggestionStatus
{
    Local,
    RemoteProposed,
    Accepted,
    Rejected,
    Applied,
    Conflict
}

public enum LlmKind
{
    Heuristic,
    OpenAi,
    Gemini,
    CustomApi,
    LocalGguf
}

public sealed record ScannedItem(
    string FullPath,
    string FileName,
    EntryKind Kind,
    FileFamily Family,
    bool IsLocked,
    string? LockReason);

public sealed record ProjectMatch(
    string RootPath,
    string Id,
    string Name,
    ProjectStrength Strength,
    string Reason)
{
    public bool ShouldSkipTraversal => Strength == ProjectStrength.Strong;
}

public sealed record ArchiveEntitySuggestion(
    string RootPath,
    string ProjectId,
    string ProjectName,
    ArchiveFormat Format,
    string SuggestedArchiveName,
    string Reason,
    ProjectStrength Strength);

public sealed record MediaMetadata
{
    public string? Year { get; init; }
    public string? Artist { get; init; }
    public string? Album { get; init; }
    public string? Title { get; init; }
    public string? Genre { get; init; }
    public int? DurationSeconds { get; init; }
    public string? Source { get; init; }

    public bool HasNamingParts =>
        !string.IsNullOrWhiteSpace(Year) ||
        !string.IsNullOrWhiteSpace(Artist) ||
        !string.IsNullOrWhiteSpace(Album) ||
        !string.IsNullOrWhiteSpace(Title);
}

public sealed record CategorizedItem
{
    public required string FullPath { get; init; }
    public required string FileName { get; init; }
    public required EntryKind Kind { get; init; }
    public required FileFamily Family { get; init; }
    public required string Category { get; init; }
    public string? Subcategory { get; init; }
    public string? SuggestedName { get; init; }
    public string? Rationale { get; init; }
    public bool IsArchiveEntity { get; init; }
    public ArchiveFormat? ArchiveFormat { get; init; }
    public bool IsLocked { get; init; }
    public string? LockReason { get; init; }
    public MediaMetadata? Media { get; init; }
    public string? LocalRelativePath { get; init; }
    public string? RemoteRelativePath { get; init; }
    public string? AcceptedRelativePath { get; init; }
    public SuggestionStatus Status { get; init; } = SuggestionStatus.Local;
    public bool Selected { get; init; } = true;

    public string EffectiveRelativePath =>
        AcceptedRelativePath ?? RemoteRelativePath ?? LocalRelativePath ?? FileName;
}

public sealed record FilingPlanSummary
{
    public int FileCount { get; init; }
    public int DirectoryCount { get; init; }
    public int ArchiveEntityCount { get; init; }
    public int LockedCount { get; init; }
    public IReadOnlyDictionary<string, int> ByFamily { get; init; } = new Dictionary<string, int>();
    public IReadOnlyDictionary<string, int> ByCategory { get; init; } = new Dictionary<string, int>();
}

public sealed record FilingPlan
{
    public const string KindId = "aifs.filingPlan.v1";

    public string Kind { get; init; } = KindId;
    public string PlanId { get; init; } = Guid.NewGuid().ToString("N");
    public DateTimeOffset CreatedAtUtc { get; init; } = DateTimeOffset.UtcNow;
    public required string RootPath { get; init; }
    public CategorizationStyle Style { get; init; } = CategorizationStyle.Refined;
    public IReadOnlyList<CategorizedItem> Entries { get; init; } = [];
    public IReadOnlyList<ArchiveEntitySuggestion> ArchiveEntities { get; init; } = [];
    public IReadOnlyList<CategorizedItem> LockedItems { get; init; } = [];
    public FilingPlanSummary Summary { get; init; } = new();
}

public sealed record ContentAnalysisOptions
{
    public bool CategorizeFiles { get; init; } = true;
    public bool CategorizeDirectories { get; init; } = true;
    public bool IncludeSubdirectories { get; init; } = true;
    public bool UseSubcategories { get; init; } = true;
    public bool AnalyzeDocuments { get; init; }
    public bool AnalyzeImages { get; init; }
    public bool AnalyzeMedia { get; init; } = true;
    public bool ProcessDocumentsOnly { get; init; }
    public bool ProcessImagesOnly { get; init; }
    public bool OfferRenameDocuments { get; init; }
    public bool OfferRenameImages { get; init; }
    public bool OfferRenameMedia { get; init; } = true;
    public bool RenameDocumentsOnly { get; init; }
    public bool RenameImagesOnly { get; init; }
    public bool AddDocumentDateToCategory { get; init; }
    public bool AddImageDateToCategory { get; init; }
    public bool AddImageDatePlaceToFilename { get; init; }
    public bool UseWhitelist { get; init; }
    public string ActiveWhitelist { get; init; } = "Unrestricted";
}

public sealed record LlmEndpointSettings
{
    public LlmKind Kind { get; init; } = LlmKind.Heuristic;
    public string OpenAiApiKey { get; init; } = "";
    public string OpenAiModel { get; init; } = "gpt-4.1-mini";
    public string GeminiApiKey { get; init; } = "";
    public string GeminiModel { get; init; } = "gemini-2.5-flash";
    public string CustomName { get; init; } = "";
    public string CustomBaseUrl { get; init; } = "";
    public string CustomApiKey { get; init; } = "";
    public string CustomModel { get; init; } = "";
    public string LocalGgufPath { get; init; } = "";
    public string LocalMmprojPath { get; init; } = "";
    public string ModelStorageDir { get; init; } = "";
    public string VisualBackendId { get; init; } = "gemma-3-4b-it";
    public string BuiltinLocalModelId { get; init; } = "gemma-3-4b-it";
}

public sealed record AnalysisRequest
{
    public required string RootPath { get; init; }
    public ScanOptions Scan { get; init; } = ScanOptions.DefaultRecursive();
    public CategorizationStyle Style { get; init; } = CategorizationStyle.Refined;
    public string? DatabasePath { get; init; }
    public ContentAnalysisOptions Content { get; init; } = new();
    public LlmEndpointSettings Llm { get; init; } = new();

    public bool AnalyzeMediaContent => Content.AnalyzeMedia;

    public bool SuggestMediaRenames => Content.OfferRenameMedia;
}

public sealed record AnalysisProgress
{
    public string Stage { get; init; } = "idle";
    public int Current { get; init; }
    public int Total { get; init; }
    public string Message { get; init; } = "";
    public double Fraction => Total <= 0 ? 0 : Math.Clamp((double)Current / Total, 0, 1);
}

public sealed record RemoteStructureSuggestion
{
    public string? ProposedRootName { get; init; }
    public string Rationale { get; init; } = "";
    public IReadOnlyList<RemoteFolderSuggestion> Folders { get; init; } = [];
    public IReadOnlyList<RemotePathUpdate> Updates { get; init; } = [];
    public string? RawResponse { get; init; }
}

public sealed record RemoteFolderSuggestion
{
    public required string Category { get; init; }
    public string? Subcategory { get; init; }
    public string? Notes { get; init; }
}

public sealed record RemotePathUpdate
{
    public string? FullPath { get; init; }
    public string? FileName { get; init; }
    public required string ProposedRelativePath { get; init; }
    public string? Category { get; init; }
    public string? Subcategory { get; init; }
    public string? Rationale { get; init; }
}

public sealed record RemoteHandoffPayload
{
    public string Kind { get; init; } = "aifs.remoteHandoff.v2";
    public required FilingPlan Plan { get; init; }
    public required string CompactPrompt { get; init; }
    public required string CompactJson { get; init; }
}

public sealed record ApplyMove(
    string SourcePath,
    string DestinationPath,
    string RelativePath,
    string FileName);

public sealed record ApplyDryRun
{
    public IReadOnlyList<ApplyMove> Moves { get; init; } = [];
    public IReadOnlyList<string> Warnings { get; init; } = [];
}

public sealed record ApplyResult
{
    public bool DryRun { get; init; }
    public IReadOnlyList<ApplyMove> Applied { get; init; } = [];
    public IReadOnlyList<string> Errors { get; init; } = [];
    public string? UndoPlanPath { get; init; }
}

public sealed record AppSettings
{
    public LlmEndpointSettings Llm { get; init; } = new();
    public ContentAnalysisOptions Content { get; init; } = new();
}
