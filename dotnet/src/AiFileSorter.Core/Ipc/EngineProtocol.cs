using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Ipc;

public sealed record EngineCommand
{
    public string Op { get; init; } = "";
    public string Id { get; init; } = "";
    public AnalysisRequest? Request { get; init; }
    public string? PlanId { get; init; }
    public string? EndpointUrl { get; init; }
    public string? ApiKey { get; init; }
    public string? Model { get; init; }
}

public sealed record EngineEvent
{
    public string Type { get; init; } = "";
    public string Id { get; init; } = "";
    public AnalysisProgress? Progress { get; init; }
    public FilingPlan? Plan { get; init; }
    public RemoteHandoffPayload? Handoff { get; init; }
    public RemoteStructureSuggestion? RemoteSuggestion { get; init; }
    public string? Error { get; init; }
}
