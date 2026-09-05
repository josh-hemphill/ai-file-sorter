using System.Text.Json;
using System.Text.Json.Serialization;
using AiFileSorter.Core.Ipc;
using AiFileSorter.Core.Llm;
using AiFileSorter.Core.Models;
using AiFileSorter.Core.Persistence;
using AiFileSorter.Core.Plans;

namespace AiFileSorter.Core.Json;

[JsonSourceGenerationOptions(
    WriteIndented = false,
    PropertyNamingPolicy = JsonKnownNamingPolicy.CamelCase,
    DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
    UseStringEnumConverter = true)]
[JsonSerializable(typeof(FilingPlan))]
[JsonSerializable(typeof(FilingPlanSummary))]
[JsonSerializable(typeof(CategorizedItem))]
[JsonSerializable(typeof(ArchiveEntitySuggestion))]
[JsonSerializable(typeof(MediaMetadata))]
[JsonSerializable(typeof(AnalysisRequest))]
[JsonSerializable(typeof(ScanOptions))]
[JsonSerializable(typeof(ContentAnalysisOptions))]
[JsonSerializable(typeof(LlmEndpointSettings))]
[JsonSerializable(typeof(AppSettings))]
[JsonSerializable(typeof(AnalysisProgress))]
[JsonSerializable(typeof(RemoteHandoffPayload))]
[JsonSerializable(typeof(RemoteStructureSuggestion))]
[JsonSerializable(typeof(RemoteFolderSuggestion))]
[JsonSerializable(typeof(RemotePathUpdate))]
[JsonSerializable(typeof(EngineCommand))]
[JsonSerializable(typeof(EngineEvent))]
[JsonSerializable(typeof(CompactHandoffDocument))]
[JsonSerializable(typeof(CompactHandoffFile))]
[JsonSerializable(typeof(RemoteChatRequest))]
[JsonSerializable(typeof(RemoteChatResponse))]
[JsonSerializable(typeof(PlanCatalog))]
[JsonSerializable(typeof(ApplyDryRun))]
[JsonSerializable(typeof(ApplyMove))]
[JsonSerializable(typeof(ApplyResult))]
[JsonSerializable(typeof(GgufDownloadMeta))]
[JsonSerializable(typeof(LlamaCompleteRequest))]
[JsonSerializable(typeof(LlamaCompleteResponse))]
[JsonSerializable(typeof(Dictionary<string, int>))]
internal partial class AppJsonContext : JsonSerializerContext;

public static class AppJson
{
    public static string Serialize<T>(T value) =>
        JsonSerializer.Serialize(value, typeof(T), AppJsonContext.Default);

    public static T Deserialize<T>(string json) =>
        (T)(JsonSerializer.Deserialize(json, typeof(T), AppJsonContext.Default)
            ?? throw new JsonException("JSON payload deserialized to null."));
}
