using System.Text;
using AiFileSorter.Core.Json;
using AiFileSorter.Core.Models;

namespace AiFileSorter.Core.Plans;

/// <summary>Builds a compact handoff payload so a remote model can propose a better filing structure.</summary>
public sealed class RemotePlanHandoff
{
    public RemoteHandoffPayload Create(FilingPlan plan)
    {
        var compact = new CompactHandoffDocument
        {
            PlanId = plan.PlanId,
            Root = plan.RootPath,
            Style = plan.Style.ToString(),
            Summary = new CompactHandoffSummary
            {
                FileCount = plan.Summary.FileCount,
                DirectoryCount = plan.Summary.DirectoryCount,
                ArchiveEntityCount = plan.Summary.ArchiveEntityCount,
                LockedCount = plan.Summary.LockedCount
            },
            ArchiveEntities = plan.ArchiveEntities.Select(entity => new CompactArchiveEntity
            {
                ProjectId = entity.ProjectId,
                ProjectName = entity.ProjectName,
                Format = entity.Format.ToString(),
                SuggestedArchiveName = entity.SuggestedArchiveName,
                Reason = entity.Reason
            }).ToArray(),
            Locked = plan.LockedItems.Select(item => item.FileName).ToArray(),
            Buckets = plan.Entries
                .GroupBy(entry => entry.Category + (string.IsNullOrWhiteSpace(entry.Subcategory) ? "" : "/" + entry.Subcategory))
                .OrderByDescending(group => group.Count())
                .Select(group => new CompactBucket
                {
                    Label = group.Key,
                    Count = group.Count(),
                    Samples = group.Take(8).Select(entry => new CompactSample
                    {
                        FileName = entry.FileName,
                        Family = entry.Family.ToString(),
                        SuggestedName = entry.SuggestedName,
                        Rationale = entry.Rationale
                    }).ToArray()
                }).ToArray(),
            Files = plan.Entries.Select(entry => new CompactHandoffFile
            {
                FullPath = entry.FullPath,
                FileName = entry.FileName,
                Family = entry.Family.ToString(),
                LocalRelativePath = entry.LocalRelativePath,
                Category = entry.Category,
                Subcategory = entry.Subcategory
            }).ToArray()
        };

        var compactJson = AppJson.Serialize(compact);
        return new RemoteHandoffPayload
        {
            Plan = plan,
            CompactJson = compactJson,
            CompactPrompt = BuildPrompt(plan, compactJson)
        };
    }

    public static RemoteStructureSuggestion ParseModelJson(string response)
    {
        var json = ExtractJsonObject(response);
        if (json is null)
        {
            return new RemoteStructureSuggestion
            {
                Rationale = "The remote model did not return JSON.",
                RawResponse = response
            };
        }

        return AppJson.Deserialize<RemoteStructureSuggestion>(json) with { RawResponse = response };
    }

    private static string BuildPrompt(FilingPlan plan, string compactJson)
    {
        var builder = new StringBuilder();
        builder.AppendLine("You are helping organize a local filesystem. Return JSON only.");
        builder.AppendLine("Do not move or rename files. Propose updated relative paths for the local suggestion database.");
        builder.AppendLine("The operator will review and apply accepted rows locally.");
        builder.AppendLine("Keep project folders listed as archive entities together as a single zip/tar item.");
        builder.AppendLine("Prefer stable category names. Prefer this schema:");
        builder.AppendLine("""{"proposedRootName":"string","rationale":"string","folders":[{"category":"string","subcategory":"string","notes":"string"}],"updates":[{"fullPath":"string","fileName":"string","proposedRelativePath":"Category/Subcategory/name.ext","category":"string","subcategory":"string","rationale":"string"}]}""");
        builder.AppendLine();
        builder.AppendLine($"Root: {plan.RootPath}");
        builder.AppendLine($"Files: {plan.Summary.FileCount}, folders: {plan.Summary.DirectoryCount}, archive entities: {plan.Summary.ArchiveEntityCount}, locked: {plan.Summary.LockedCount}");
        builder.AppendLine("Plan JSON:");
        builder.AppendLine(compactJson);
        return builder.ToString();
    }

    private static string? ExtractJsonObject(string response)
    {
        var start = response.IndexOf('{');
        var end = response.LastIndexOf('}');
        if (start < 0 || end <= start)
        {
            return null;
        }

        return response[start..(end + 1)];
    }
}

public interface IRemotePlanClient
{
    Task<RemoteStructureSuggestion> SuggestStructureAsync(
        RemoteHandoffPayload payload,
        CancellationToken cancellationToken = default);
}

/// <summary>Posts the compact plan to an OpenAI-compatible chat completions endpoint.</summary>
public sealed class OpenAiCompatiblePlanClient : IRemotePlanClient
{
    private readonly HttpClient _httpClient;
    private readonly string _endpointUrl;
    private readonly string? _apiKey;
    private readonly string _model;

    public OpenAiCompatiblePlanClient(HttpClient httpClient, string endpointUrl, string model, string? apiKey)
    {
        _httpClient = httpClient;
        _endpointUrl = endpointUrl.TrimEnd('/');
        _model = model;
        _apiKey = apiKey;
    }

    public async Task<RemoteStructureSuggestion> SuggestStructureAsync(
        RemoteHandoffPayload payload,
        CancellationToken cancellationToken = default)
    {
        var url = _endpointUrl.EndsWith("/chat/completions", StringComparison.OrdinalIgnoreCase)
            ? _endpointUrl
            : _endpointUrl + "/chat/completions";

        using var request = new HttpRequestMessage(HttpMethod.Post, url);
        if (!string.IsNullOrWhiteSpace(_apiKey))
        {
            request.Headers.Authorization = new System.Net.Http.Headers.AuthenticationHeaderValue("Bearer", _apiKey);
        }

        var body = new RemoteChatRequest
        {
            Model = _model,
            Temperature = 0.2,
            Messages =
            [
                new RemoteChatMessage { Role = "system", Content = "Return JSON only. No markdown." },
                new RemoteChatMessage { Role = "user", Content = payload.CompactPrompt }
            ]
        };

        request.Content = new StringContent(AppJson.Serialize(body), Encoding.UTF8, "application/json");
        using var response = await _httpClient.SendAsync(request, cancellationToken).ConfigureAwait(false);
        var text = await response.Content.ReadAsStringAsync(cancellationToken).ConfigureAwait(false);
        if (!response.IsSuccessStatusCode)
        {
            throw new HttpRequestException($"Remote model returned {(int)response.StatusCode}: {text}");
        }

        var completion = AppJson.Deserialize<RemoteChatResponse>(text);
        var content = completion.Choices is { Count: > 0 } ? completion.Choices[0].Message?.Content : null;
        return RemotePlanHandoff.ParseModelJson(content ?? text);
    }
}
