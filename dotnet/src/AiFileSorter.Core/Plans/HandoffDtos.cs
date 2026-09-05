namespace AiFileSorter.Core.Plans;

public sealed record CompactHandoffDocument
{
    public string Kind { get; init; } = "aifs.remoteHandoff.v1";
    public string PlanId { get; init; } = "";
    public string Root { get; init; } = "";
    public string Style { get; init; } = "";
    public required CompactHandoffSummary Summary { get; init; }
    public IReadOnlyList<CompactArchiveEntity> ArchiveEntities { get; init; } = [];
    public IReadOnlyList<string> Locked { get; init; } = [];
    public IReadOnlyList<CompactBucket> Buckets { get; init; } = [];
}

public sealed record CompactHandoffSummary
{
    public int FileCount { get; init; }
    public int DirectoryCount { get; init; }
    public int ArchiveEntityCount { get; init; }
    public int LockedCount { get; init; }
}

public sealed record CompactArchiveEntity
{
    public string ProjectId { get; init; } = "";
    public string ProjectName { get; init; } = "";
    public string Format { get; init; } = "";
    public string SuggestedArchiveName { get; init; } = "";
    public string Reason { get; init; } = "";
}

public sealed record CompactBucket
{
    public string Label { get; init; } = "";
    public int Count { get; init; }
    public IReadOnlyList<CompactSample> Samples { get; init; } = [];
}

public sealed record CompactSample
{
    public string FileName { get; init; } = "";
    public string Family { get; init; } = "";
    public string? SuggestedName { get; init; }
    public string? Rationale { get; init; }
}

public sealed record RemoteChatRequest
{
    public string Model { get; set; } = "";
    public double Temperature { get; set; }
    public IReadOnlyList<RemoteChatMessage> Messages { get; set; } = [];
}

public sealed record RemoteChatMessage
{
    public string Role { get; set; } = "";
    public string Content { get; set; } = "";
}

public sealed record RemoteChatResponse
{
    public IReadOnlyList<RemoteChatChoice>? Choices { get; set; }
}

public sealed record RemoteChatChoice
{
    public RemoteChatMessage? Message { get; set; }
}
