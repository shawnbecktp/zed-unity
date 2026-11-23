using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Text;

var input = Console.OpenStandardInput();
var output = Console.OpenStandardOutput();
var documents = new Dictionary<string, string>();
var index = new EventIndex();

IEnumerable<AssetUse> References(JsonNode args)
{
    var uri = (string)args["textDocument"]!["uri"]!;
    if (!Uri.TryCreate(uri, UriKind.Absolute, out var file) || !file.IsFile ||
        !documents.TryGetValue(uri, out var code)) return [];
    var path = file.LocalPath;
    var root = new FileInfo(path).Directory;
    while (root != null && !Directory.Exists(Path.Combine(root.FullName, "Assets")))
        root = root.Parent;
    if (root == null || !File.Exists(path + ".meta")) return [];
    var source = SourceText.From(code);
    int line = (int)args["position"]!["line"]!, col = (int)args["position"]!["character"]!;
    if (line < 0 || line >= source.Lines.Count || col < 0 || col >= source.Lines[line].Span.Length) return [];
    var token = CSharpSyntaxTree.ParseText(source).GetRoot().FindToken(source.Lines[line].Start + col);
    string? methodName = null;
    ClassDeclarationSyntax type;
    if (token.Parent is ClassDeclarationSyntax declaration && token == declaration.Identifier)
        type = declaration;
    else if (token.Parent is MethodDeclarationSyntax method && token == method.Identifier &&
             method.Parent is ClassDeclarationSyntax owner &&
             owner.Members.OfType<MethodDeclarationSyntax>().Count(m => m.Identifier.ValueText == token.ValueText) == 1)
    {
        type = owner;
        methodName = token.ValueText;
    }
    else return [];
    if (type.Parent is TypeDeclarationSyntax || type.Identifier.ValueText != Path.GetFileNameWithoutExtension(path)) return [];
    var guid = Regex.Match(File.ReadAllText(path + ".meta"), @"(?m)^guid: ([a-fA-F0-9]{32})\s*$");
    return guid.Success ? index.Find(Path.Combine(root.FullName, "Assets"), guid.Groups[1].Value, methodName) : [];
}

while (ReadMessage(input) is JsonObject request)
{
    var method = (string?)request["method"];
    var argsNode = request["params"];
    object? result = null;
    object? error = null;
    try
    {
        switch (method)
        {
            case "initialize":
                result = new { capabilities = new { textDocumentSync = 1, referencesProvider = true, hoverProvider = true } };
                break;
            case "textDocument/didOpen":
                var doc = argsNode!["textDocument"]!;
                documents[(string)doc["uri"]!] = (string)doc["text"]!;
                break;
            case "textDocument/didChange":
                documents[(string)argsNode!["textDocument"]!["uri"]!] = (string)argsNode["contentChanges"]!.AsArray().Last()!["text"]!;
                break;
            case "textDocument/didClose":
                documents.Remove((string)argsNode!["textDocument"]!["uri"]!);
                break;
            case "textDocument/references": result = References(argsNode!).Select(hit => hit.Location).ToArray(); break;
            case "textDocument/hover":
                var sourceFile = new Uri((string)argsNode!["textDocument"]!["uri"]!).LocalPath;
                var objects = References(argsNode).Select(hit =>
                    $"{hit.Use.Owner} — {Path.GetRelativePath(Path.GetDirectoryName(sourceFile)!, hit.Path)}:{hit.Use.Line + 1}")
                    .OrderBy(label => label, StringComparer.Ordinal).ToArray();
                if (objects.Length > 0)
                    result = new { contents = new { kind = "plaintext", value = "Unity GameObjects\n" + string.Join("\n", objects) } };
                break;
            case "shutdown": break;
            case "exit": return;
            default: error = new { code = -32601, message = "Unsupported method" }; break;
        }
    }
    catch (Exception ex)
    {
        error = new { code = -32603, message = ex.Message };
        Console.Error.WriteLine(ex);
    }
    if (request["id"] != null && method != null)
    {
        var id = request["id"];
        object response = error == null ? new { jsonrpc = "2.0", id, result } : new { jsonrpc = "2.0", id, error };
        var bytes = JsonSerializer.SerializeToUtf8Bytes(response);
        output.Write(Encoding.ASCII.GetBytes($"Content-Length: {bytes.Length}\r\n\r\n"));
        output.Write(bytes);
        output.Flush();
    }
}

static JsonNode? ReadMessage(Stream input)
{
    var header = new StringBuilder();
    int length = -1;
    while (true)
    {
        int b = input.ReadByte();
        if (b < 0) return null;
        if (b != '\n') { header.Append((char)b); continue; }
        var line = header.ToString().Trim();
        header.Clear();
        if (line.Length == 0) break;
        if (line.StartsWith("Content-Length:", StringComparison.OrdinalIgnoreCase))
            length = int.Parse(line["Content-Length:".Length..]);
    }
    if (length < 0) throw new InvalidDataException("Missing Content-Length");
    var body = new byte[length];
    input.ReadExactly(body);
    return JsonNode.Parse(body);
}
