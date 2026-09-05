using System.Linq;
using Avalonia.Controls;
using Avalonia.Interactivity;
using Avalonia.Platform.Storage;
using AiFileSorter.App.ViewModels;

namespace AiFileSorter.App.Views;

public partial class MainWindow : Window
{
    public MainWindow()
    {
        InitializeComponent();
    }

    private MainWindowViewModel? ViewModel => DataContext as MainWindowViewModel;

    private async void OnBrowseClicked(object? sender, RoutedEventArgs e)
    {
        if (ViewModel is null)
        {
            return;
        }

        var folders = await StorageProvider.OpenFolderPickerAsync(new FolderPickerOpenOptions
        {
            Title = "Select a folder to analyze",
            AllowMultiple = false
        }).ConfigureAwait(true);

        var path = folders.FirstOrDefault()?.TryGetLocalPath();
        if (!string.IsNullOrWhiteSpace(path))
        {
            ViewModel.SetFolder(path);
        }
    }

    private async void OnModelsClicked(object? sender, RoutedEventArgs e)
    {
        if (ViewModel is null)
        {
            return;
        }

        var dialog = new ModelsWindow
        {
            DataContext = ViewModel.Llm
        };
        await dialog.ShowDialog(this).ConfigureAwait(true);
        ViewModel.SaveLlmSettings();
    }

    private async void OnImportProposalClicked(object? sender, RoutedEventArgs e)
    {
        if (ViewModel is null)
        {
            return;
        }

        var files = await StorageProvider.OpenFilePickerAsync(new FilePickerOpenOptions
        {
            Title = "Import remote path proposal JSON",
            AllowMultiple = false,
            FileTypeFilter =
            [
                new FilePickerFileType("JSON") { Patterns = ["*.json"] }
            ]
        }).ConfigureAwait(true);

        var path = files.FirstOrDefault()?.TryGetLocalPath();
        if (string.IsNullOrWhiteSpace(path))
        {
            return;
        }

        try
        {
            var json = await System.IO.File.ReadAllTextAsync(path).ConfigureAwait(true);
            var proposal = Core.Plans.RemotePlanHandoff.ParseModelJson(json);
            var merged = new Core.Engine.AnalysisEngine().MergeRemote(
                ViewModel.PlanFromRows(),
                proposal,
                Core.Engine.AnalysisEngine.DefaultDatabasePath());
            ViewModel.ApplyPlan(merged);
            ViewModel.Status = "Imported remote path proposals into the local database. Review Proposed path, then apply locally.";
        }
        catch (System.Exception ex)
        {
            ViewModel.Status = "Import failed: " + ex.Message;
        }
    }
}
