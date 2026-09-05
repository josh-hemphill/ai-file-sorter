using System.Text;
using AiFileSorter.Core.Json;
using AiFileSorter.Core.Llm;
using LLama;
using LLama.Common;
using LLama.Sampling;

namespace AiFileSorter.Llama;

public static class Program
{
    public static async Task<int> Main(string[] args)
    {
        if (args.Length == 0 || args[0] is "-h" or "--help" or "help")
        {
            Console.WriteLine("aifs-llama complete   # JSON LlamaCompleteRequest on stdin");
            return 0;
        }

        if (args[0] != "complete")
        {
            Console.Error.WriteLine("Unknown command. Use: aifs-llama complete");
            return 1;
        }

        try
        {
            var json = await Console.In.ReadToEndAsync().ConfigureAwait(false);
            var request = AppJson.Deserialize<LlamaCompleteRequest>(json);
            var text = await CompleteAsync(request).ConfigureAwait(false);
            Console.WriteLine(AppJson.Serialize(new LlamaCompleteResponse { Text = text }));
            return 0;
        }
        catch (Exception ex)
        {
            Console.WriteLine(AppJson.Serialize(new LlamaCompleteResponse { Error = ex.Message }));
            return 1;
        }
    }

    private static async Task<string> CompleteAsync(LlamaCompleteRequest request)
    {
        var parameters = new ModelParams(request.ModelPath)
        {
            ContextSize = 2048,
            GpuLayerCount = 0
        };
        using var weights = LLamaWeights.LoadFromFile(parameters);
        var executor = new StatelessExecutor(weights, parameters);
        var inference = new InferenceParams
        {
            MaxTokens = Math.Clamp(request.MaxTokens, 16, 1024),
            SamplingPipeline = new DefaultSamplingPipeline { Temperature = 0.2f }
        };
        var prompt = new StringBuilder();
        if (!string.IsNullOrWhiteSpace(request.SystemPrompt))
        {
            prompt.AppendLine(request.SystemPrompt);
            prompt.AppendLine();
        }

        prompt.AppendLine(request.UserPrompt);
        var output = new StringBuilder();
        await foreach (var chunk in executor.InferAsync(prompt.ToString(), inference).ConfigureAwait(false))
        {
            output.Append(chunk);
        }

        return output.ToString().Trim();
    }
}
