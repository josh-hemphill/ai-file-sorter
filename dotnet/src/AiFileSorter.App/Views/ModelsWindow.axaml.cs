using Avalonia.Controls;
using Avalonia.Interactivity;

namespace AiFileSorter.App.Views;

public partial class ModelsWindow : Window
{
    public ModelsWindow()
    {
        InitializeComponent();
    }

    private void OnCloseClicked(object? sender, RoutedEventArgs e) => Close();
}
