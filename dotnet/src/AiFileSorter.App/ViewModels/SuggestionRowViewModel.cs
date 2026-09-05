using AiFileSorter.Core.Models;
using CommunityToolkit.Mvvm.ComponentModel;

namespace AiFileSorter.App.ViewModels;

public sealed partial class SuggestionRowViewModel : ObservableObject
{
    public SuggestionRowViewModel(CategorizedItem item)
    {
        Item = item;
        selected = item.Selected;
        proposedPath = item.EffectiveRelativePath;
        status = item.Status;
        category = item.Category;
        subcategory = item.Subcategory ?? "";
        notes = item.Rationale ?? "";
    }

    public CategorizedItem Item { get; private set; }

    public string FileName => Item.FileName;
    public FileFamily Family => Item.Family;
    public EntryKind Kind => Item.Kind;

    [ObservableProperty]
    private bool selected;

    [ObservableProperty]
    private string proposedPath;

    [ObservableProperty]
    private SuggestionStatus status;

    [ObservableProperty]
    private string category;

    [ObservableProperty]
    private string subcategory;

    [ObservableProperty]
    private string notes;

    public string LocalRelativePath => Item.LocalRelativePath ?? "";
    public string RemoteRelativePath => Item.RemoteRelativePath ?? "";

    public CategorizedItem ToItem() => Item with
    {
        Selected = Selected,
        Category = Category,
        Subcategory = string.IsNullOrWhiteSpace(Subcategory) ? null : Subcategory,
        Rationale = string.IsNullOrWhiteSpace(Notes) ? null : Notes,
        AcceptedRelativePath = ProposedPath,
        Status = Status,
        RemoteRelativePath = Item.RemoteRelativePath,
        LocalRelativePath = Item.LocalRelativePath
    };

    public void AcceptRemote()
    {
        if (string.IsNullOrWhiteSpace(Item.RemoteRelativePath))
        {
            return;
        }

        ProposedPath = Item.RemoteRelativePath;
        Status = SuggestionStatus.Accepted;
    }

    public void KeepLocal()
    {
        ProposedPath = Item.LocalRelativePath ?? Item.FileName;
        Status = SuggestionStatus.Accepted;
    }

    public void RejectRemote()
    {
        ProposedPath = Item.LocalRelativePath ?? Item.FileName;
        Status = SuggestionStatus.Rejected;
    }

    partial void OnProposedPathChanged(string value)
    {
        if (Status is SuggestionStatus.Rejected or SuggestionStatus.Applied)
        {
            return;
        }

        Status = SuggestionStatus.Accepted;
    }
}
