using System.Threading;
using System.Threading.Tasks;
using AiFileSorter.Core.Llm;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace AiFileSorter.App.ViewModels;

public sealed partial class GgufDownloadItemViewModel : ObservableObject
{
    private readonly LlmSettingsViewModel _owner;
    private CancellationTokenSource? _downloadCts;

    public GgufDownloadItemViewModel(GgufArtifact artifact, LlmSettingsViewModel owner)
    {
        Artifact = artifact;
        _owner = owner;
        functionsLabel = GgufCatalog.FormatFunctions(artifact);
        Refresh();
    }

    public GgufArtifact Artifact { get; }
    public string DisplayName => Artifact.ResolveDisplayName();
    public string ArtifactId => Artifact.Id;

    [ObservableProperty]
    private string functionsLabel;

    [ObservableProperty]
    private string remoteUrl = "";

    [ObservableProperty]
    private string localPath = "";

    [ObservableProperty]
    private string statusText = "Not downloaded";

    [ObservableProperty]
    private string sizeText = "";

    [ObservableProperty]
    private double progress;

    [ObservableProperty]
    private bool isDownloading;

    [ObservableProperty]
    private bool canDownload = true;

    [ObservableProperty]
    private bool canDelete;

    [ObservableProperty]
    private bool isComplete;

    public bool IsSelected
    {
        get => string.Equals(_owner.BuiltinLocalModelId, Artifact.Id, StringComparison.Ordinal);
        set
        {
            if (value)
            {
                _owner.SelectModel(Artifact.Id);
            }

            OnPropertyChanged();
        }
    }

    public void NotifySelected() => OnPropertyChanged(nameof(IsSelected));

    public void Refresh()
    {
        var probe = _owner.Downloader.Probe(Artifact, _owner.ModelStorageDir);
        RemoteUrl = probe.Url;
        LocalPath = probe.Path;
        IsComplete = probe.State == GgufLocalState.Complete;
        CanDelete = probe.State is GgufLocalState.Complete or GgufLocalState.Partial or GgufLocalState.Corrupt;
        CanDownload = !IsDownloading && probe.State is not GgufLocalState.MissingUrl and not GgufLocalState.Complete;
        SizeText = probe.BytesOnDisk > 0
            ? GgufDownloader.FormatSize(probe.BytesOnDisk) + (probe.ExpectedBytes is > 0 ? " / " + GgufDownloader.FormatSize(probe.ExpectedBytes.Value) : "")
            : "";
        StatusText = probe.State switch
        {
            GgufLocalState.Complete => probe.Message,
            GgufLocalState.Partial => probe.Message,
            GgufLocalState.Corrupt => probe.Message,
            GgufLocalState.MissingUrl => probe.Message,
            _ => "Not downloaded — " + FunctionsLabel
        };
        if (!IsDownloading)
        {
            Progress = probe.State == GgufLocalState.Complete ? 1 : probe.ExpectedBytes is > 0 ? (double)probe.BytesOnDisk / probe.ExpectedBytes.Value : 0;
        }
    }

    [RelayCommand]
    private async Task DownloadAsync()
    {
        _downloadCts?.Cancel();
        _downloadCts?.Dispose();
        _downloadCts = new CancellationTokenSource();
        IsDownloading = true;
        CanDownload = false;
        StatusText = "Starting download…";
        try
        {
            var progress = new Progress<GgufDownloadProgress>(update =>
            {
                Progress = update.Fraction;
                StatusText = update.Status;
                if (update.BytesReceived > 0)
                {
                    SizeText = GgufDownloader.FormatSize(update.BytesReceived) +
                               (update.TotalBytes is > 0 ? " / " + GgufDownloader.FormatSize(update.TotalBytes.Value) : "");
                }
            });
            await _owner.Downloader.DownloadAsync(Artifact, _owner.ModelStorageDir, progress, _downloadCts.Token).ConfigureAwait(true);
            StatusText = "Downloaded";
            _owner.OnDownloadFinished();
        }
        catch (OperationCanceledException)
        {
            StatusText = "Download cancelled";
        }
        catch (Exception ex)
        {
            StatusText = "Download failed: " + ex.Message;
        }
        finally
        {
            IsDownloading = false;
            Refresh();
        }
    }

    [RelayCommand]
    private void Cancel()
    {
        _downloadCts?.Cancel();
    }

    [RelayCommand]
    private void Delete()
    {
        _owner.Downloader.Delete(Artifact, _owner.ModelStorageDir);
        Refresh();
        _owner.OnDownloadFinished();
    }
}
