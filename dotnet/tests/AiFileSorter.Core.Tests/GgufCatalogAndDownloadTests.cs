using System.Net;
using System.Net.Http.Headers;
using AiFileSorter.Core.Engine;
using AiFileSorter.Core.Llm;
using AiFileSorter.Core.Models;
using AiFileSorter.Core.Plans;
using Xunit;

namespace AiFileSorter.Core.Tests;

public sealed class GgufCatalogAndDownloadTests
{
    [Fact]
    public void Catalog_covers_categorization_documents_and_image_analysis()
    {
        Assert.Contains(GgufCatalog.CategorizationModels, model => model.Id == "gemma-3-4b-it");
        Assert.Contains(GgufCatalog.CategorizationModels, model => model.DisplayName.Contains("Mistral 7B"));
        Assert.All(GgufCatalog.CategorizationModels, model =>
        {
            Assert.Contains(GgufFunction.Categorization, model.Artifact.Functions);
            Assert.Contains(GgufFunction.Documents, model.Artifact.Functions);
            Assert.Contains("huggingface.co", model.Artifact.DefaultUrl);
        });
        Assert.Equal(2, GgufCatalog.VisualBackends.Count);
        Assert.All(GgufCatalog.VisualBackends, backend =>
        {
            Assert.Equal(GgufFunction.ImageAnalysis, backend.TextModel.Functions[0]);
            Assert.Equal(GgufArtifactKind.Mmproj, backend.Mmproj.Kind);
        });
        Assert.Equal(
            "https://huggingface.co/ggml-org/gemma-3-4b-it-GGUF/resolve/main/gemma-3-4b-it-Q4_K_M.gguf",
            GgufCatalog.Gemma3Categorization.DefaultUrl);
    }

    [Fact]
    public void Storage_default_matches_qt_linux_path()
    {
        if (!OperatingSystem.IsLinux())
        {
            return;
        }

        var home = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        Assert.Equal(Path.Combine(home, ".local", "share", "aifilesorter", "llms"), GgufStorage.DefaultDirectory());
    }

    [Fact]
    public void Validates_gguf_magic_and_rejects_corrupt_files()
    {
        using var root = new TempFolder();
        var good = Path.Combine(root.Path, "ok.gguf");
        var bad = Path.Combine(root.Path, "bad.gguf");
        GgufFileValidation.WriteTinyFixture(good);
        File.WriteAllText(bad, "not a model");
        Assert.True(GgufFileValidation.HasGgufHeader(good));
        Assert.False(GgufFileValidation.HasGgufHeader(bad));
    }

    [Fact]
    public async Task Downloader_resumes_and_validates_gguf()
    {
        var payload = new byte[8 + 64];
        "GGUF"u8.CopyTo(payload);
        using var handler = new ScriptedGgufHandler(payload);
        using var downloader = new GgufDownloader(handler, disposeHandler: false);
        using var root = new TempFolder();
        var artifact = new GgufArtifact
        {
            Id = "fixture",
            DisplayName = "Fixture",
            UrlEnv = "AIFS_TEST_GGUF_URL",
            DefaultUrl = "http://models.test/fixture.gguf",
            RelativePath = "fixture.gguf",
            Kind = GgufArtifactKind.TextModel,
            Functions = [GgufFunction.Categorization]
        };

        var dest = GgufStorage.DestinationPath(artifact, root.Path);
        Directory.CreateDirectory(Path.GetDirectoryName(dest)!);
        await File.WriteAllBytesAsync(GgufStorage.PartialPath(dest), payload.AsMemory(0, 10).ToArray());

        await downloader.DownloadAsync(artifact, root.Path, progress: null);
        var probe = downloader.Probe(artifact, root.Path);
        Assert.Equal(GgufLocalState.Complete, probe.State);
        Assert.True(GgufFileValidation.HasGgufHeader(dest));
        Assert.Equal(1, handler.RangeRequests);
    }

    [Fact]
    public void Parses_local_llm_category_line()
    {
        Assert.True(LocalLlmPrompts.TryParseCategoryLine("Documents : Meeting Notes\n", out var category, out var subcategory));
        Assert.Equal("Documents", category);
        Assert.Equal("Meeting Notes", subcategory);
    }

    [Fact]
    public async Task Local_gguf_categorizer_overrides_heuristic_when_sidecar_replies()
    {
        using var root = new TempFolder();
        using var models = new TempFolder();
        File.WriteAllText(Path.Combine(root.Path, "invoice.txt"), "pay this invoice");
        var model = Path.Combine(models.Path, "tiny.gguf");
        GgufFileValidation.WriteTinyFixture(model);
        var fake = new ScriptedLlamaClient("Finance : Invoices");
        var engine = new AnalysisEngine(
            new Core.Scanning.FileScanner(),
            new FilingPlanBuilder(),
            new RemotePlanHandoff(),
            new ApplyService(),
            fake);
        var plan = await engine.AnalyzeAsync(new AnalysisRequest
        {
            RootPath = root.Path,
            Scan = ScanOptions.DefaultRecursive(),
            Content = new ContentAnalysisOptions { AnalyzeDocuments = true, AnalyzeMedia = false },
            Llm = new LlmEndpointSettings { Kind = LlmKind.LocalGguf, LocalGgufPath = model }
        }, progress: null);

        Assert.Contains(plan.Entries, entry => entry.FileName == "invoice.txt" && entry.Category == "Finance" && entry.Subcategory == "Invoices");
        Assert.Equal(1, fake.Calls);
    }
}

public sealed class ScriptedLlamaClient : ILocalLlmClient
{
    private readonly string _reply;

    public ScriptedLlamaClient(string reply)
    {
        _reply = reply;
    }

    public bool IsAvailable => true;
    public string Source => "scripted";
    public int Calls { get; private set; }

    public string Complete(LlamaCompleteRequest request, CancellationToken cancellationToken = default)
    {
        Calls++;
        return _reply;
    }
}

public sealed class ScriptedGgufHandler : HttpMessageHandler
{
    private readonly byte[] _payload;

    public ScriptedGgufHandler(byte[] payload)
    {
        _payload = payload;
    }

    public int RangeRequests { get; private set; }

    protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
    {
        var offset = 0;
        if (request.Headers.Range?.Ranges.FirstOrDefault() is { From: { } from })
        {
            offset = (int)from;
            RangeRequests++;
        }

        var slice = _payload.AsMemory(offset);
        var response = new HttpResponseMessage(offset == 0 ? HttpStatusCode.OK : HttpStatusCode.PartialContent)
        {
            Content = new ByteArrayContent(slice.ToArray())
        };
        response.Content.Headers.ContentLength = slice.Length;
        if (offset > 0)
        {
            response.Content.Headers.ContentRange = new ContentRangeHeaderValue(offset, _payload.Length - 1, _payload.Length);
        }

        return Task.FromResult(response);
    }
}
