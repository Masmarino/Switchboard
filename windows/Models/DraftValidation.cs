using System.Text.Json.Serialization;

namespace Switchboard.Models;

public class DraftValidation
{
    [JsonPropertyName("errors")]
    public List<FieldIssue> Errors { get; set; } = [];

    [JsonPropertyName("warnings")]
    public List<FieldIssue> Warnings { get; set; } = [];
}
