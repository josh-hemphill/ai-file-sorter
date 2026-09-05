using System.Linq;
using Avalonia.Controls;
using Avalonia.Interactivity;
using Avalonia.Platform.Storage;
using AiFileSorter.App.ViewModels;

namespace AiFileSorter.App.Views;

public partial class ModelsWindow : Window
{
    public ModelsWindow()
    {
        InitializeComponent();
    }

    private void OnCloseClicked(object? sender, RoutedEventArgs e) => Close();

    private async void OnBrowseStorageClicked(object? sender, RoutedEventArgs e)
    {
        if (DataContext is not LlmSettingsViewModel viewModel)
        {
            return;
        }

        var folders = await StorageProvider.OpenFolderPickerAsync(new FolderPickerOpenOptions
        {
            Title = "Local LLM storage directory",
            AllowMultiple = false
        }).ConfigureAwait(true);

        var path = folders.FirstOrDefault()?.TryGetLocalPath();
        if (!string.IsNullOrWhiteSpace(path))
        {
            viewModel.SetStorageDir(path);
        }
    }
}
