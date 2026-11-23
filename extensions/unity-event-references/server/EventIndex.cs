using System.Text.RegularExpressions;

record UnityUse(string ScriptGuid, string? Method, string Owner, int Line, int Column, int Length);
record AssetUse(string Path, UnityUse Use)
{
    public object Location => new {
        uri = new Uri(Path).AbsoluteUri,
        range = new {
            start = new { line = Use.Line, character = Use.Column },
            end = new { line = Use.Line, character = Use.Column + Use.Length }
        }
    };
}

class EventIndex
{
    private readonly Dictionary<string, (long Size, DateTime Modified, List<UnityUse> Calls)> cache = new();
    private static readonly Regex Header = new(@"^--- !u!(\d+) &(-?\d+)", RegexOptions.Compiled);
    private static readonly Regex Script = new(@"^  m_Script: \{fileID: 11500000, guid: ([0-9a-fA-F]{32}),", RegexOptions.Compiled);
    private static readonly Regex GameObject = new(@"^  m_GameObject: \{fileID: (-?\d+)\}$", RegexOptions.Compiled);
    private static readonly Regex Target = new(@"^- m_Target: \{fileID: (-?\d+)\}$", RegexOptions.Compiled);

    // Read Unity's text serialization. Local fileIDs are scoped to one asset.
    private static List<UnityUse> Read(string path)
    {
        var components = new Dictionary<string, string>();
        var owners = new Dictionary<string, string>();
        var names = new Dictionary<string, string>();
        var scripts = new List<(string Component, string Guid, int Line, int Column)>();
        var calls = new List<(string Component, string Target, string Name, int Line, int Column)>();
        using var reader = File.OpenText(path);
        if (reader.ReadLine()?.StartsWith("%YAML ") != true) return [];
        string? component = null, gameObject = null, target = null;
        int persistentIndent = -1, callsIndent = -1, line = 0;
        while (reader.ReadLine() is string raw)
        {
            line++;
            var text = raw.TrimStart(' ');
            int indent = raw.Length - text.Length;
            if (text.Length == 0) continue;
            if (raw.StartsWith("---"))
            {
                var doc = Header.Match(raw);
                gameObject = doc.Groups[1].Value == "1" ? doc.Groups[2].Value : null;
                component = doc.Groups[1].Value == "114" ? doc.Groups[2].Value : null;
                persistentIndent = callsIndent = -1;
                target = null;
                continue;
            }
            if (gameObject != null && raw.StartsWith("  m_Name: "))
                names[gameObject] = raw["  m_Name: ".Length..].Trim();
            if (component == null) continue;
            if (GameObject.Match(raw) is { Success: true } owner)
                owners[component] = owner.Groups[1].Value;
            if (raw.StartsWith("  m_Script:") && Script.Match(raw) is { Success: true } script)
            {
                var guid = script.Groups[1];
                components[component] = guid.Value;
                scripts.Add((component, guid.Value, line, guid.Index));
            }
            if (indent <= persistentIndent)
                persistentIndent = callsIndent = -1;
            if (text == "m_PersistentCalls:")
            {
                persistentIndent = indent;
                callsIndent = -1;
                target = null;
            }
            else if (persistentIndent >= 0 && indent == persistentIndent + 2 && text == "m_Calls:")
            {
                callsIndent = indent;
                target = null;
            }
            else if (callsIndent >= 0 && indent == callsIndent && text.StartsWith("- "))
            {
                var match = Target.Match(text);
                target = match.Success ? match.Groups[1].Value : null;
            }
            else if (indent <= callsIndent)
            {
                callsIndent = -1;
                target = null;
            }
            else if (callsIndent >= 0 && indent == callsIndent + 2 && target != null && text.StartsWith("m_MethodName: "))
            {
                var name = text["m_MethodName: ".Length..].TrimEnd();
                calls.Add((component, target, name, line, indent + "m_MethodName: ".Length));
            }
        }
        string Owner(string componentId) =>
            owners.TryGetValue(componentId, out var id)
                ? names.GetValueOrDefault(id, $"GameObject {id}") : $"Component {componentId}";
        return scripts.Select(s => new UnityUse(s.Guid, null, Owner(s.Component), s.Line, s.Column, s.Guid.Length))
            .Concat(calls.Where(c => components.ContainsKey(c.Target))
                .Select(c => new UnityUse(components[c.Target], c.Name, Owner(c.Component), c.Line, c.Column, c.Name.Length)))
            .ToList();
    }

    public IEnumerable<AssetUse> Find(string assets, string guid, string? method)
    {
        var results = new List<AssetUse>();
        var seen = new HashSet<string>();
        var options = new EnumerationOptions { RecurseSubdirectories = true, AttributesToSkip = FileAttributes.ReparsePoint };
        foreach (var file in new DirectoryInfo(assets).EnumerateFiles("*", options))
        {
            if (file.Extension is not (".unity" or ".prefab")) continue;
            var path = file.FullName;
            seen.Add(path);
            try
            {
                if (!cache.TryGetValue(path, out var entry) || entry.Size != file.Length || entry.Modified != file.LastWriteTimeUtc)
                    cache[path] = entry = (file.Length, file.LastWriteTimeUtc, Read(path));
                foreach (var call in entry.Calls.Where(c => StringComparer.OrdinalIgnoreCase.Equals(c.ScriptGuid, guid) && c.Method == method))
                    results.Add(new AssetUse(path, call));
            }
            catch (IOException) { cache.Remove(path); }
            catch (UnauthorizedAccessException) { cache.Remove(path); }
        }
        foreach (var removed in cache.Keys.Except(seen).ToArray()) cache.Remove(removed);
        return results;
    }

}
