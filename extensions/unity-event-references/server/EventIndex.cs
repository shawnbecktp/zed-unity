using System.Text.RegularExpressions;

record Listener(string ScriptGuid, string Method, int Line, int Column);

class EventIndex
{
    private readonly Dictionary<string, (long Size, DateTime Modified, List<Listener> Calls)> cache = new();
    private static readonly Regex Header = new(@"^--- !u!(\d+) &(-?\d+)", RegexOptions.Compiled);
    private static readonly Regex Script = new(@"^  m_Script: \{fileID: 11500000, guid: ([0-9a-fA-F]{32}),", RegexOptions.Compiled);
    private static readonly Regex Target = new(@"^- m_Target: \{fileID: (-?\d+)\}$", RegexOptions.Compiled);

    // Read Unity's text serialization. Local fileIDs are scoped to one asset.
    private static List<Listener> Read(string path)
    {
        var components = new Dictionary<string, string>();
        var calls = new List<(string Target, string Name, int Line, int Column)>();
        using var reader = File.OpenText(path);
        if (reader.ReadLine()?.StartsWith("%YAML ") != true) return [];
        string? component = null, target = null;
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
                component = doc.Groups[1].Value == "114" ? doc.Groups[2].Value : null;
                persistentIndent = callsIndent = -1;
                target = null;
                continue;
            }
            if (component == null) continue;
            if (raw.StartsWith("  m_Script:") && Script.Match(raw) is { Success: true } script)
                components[component] = script.Groups[1].Value;
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
                calls.Add((target, name, line, indent + "m_MethodName: ".Length));
            }
        }
        return calls.Where(c => components.ContainsKey(c.Target))
            .Select(c => new Listener(components[c.Target], c.Name, c.Line, c.Column)).ToList();
    }

    public IEnumerable<object> Find(string assets, string guid, string method)
    {
        var results = new List<object>();
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
                foreach (var call in entry.Calls.Where(c => c.ScriptGuid == guid && c.Method == method).ToList())
                    results.Add(Location(path, call));
            }
            catch (IOException) { cache.Remove(path); }
            catch (UnauthorizedAccessException) { cache.Remove(path); }
        }
        foreach (var removed in cache.Keys.Except(seen).ToArray()) cache.Remove(removed);
        return results;
    }

    private static object Location(string path, Listener c) => new {
        uri = new Uri(path).AbsoluteUri,
        range = new {
            start = new { line = c.Line, character = c.Column },
            end = new { line = c.Line, character = c.Column + c.Method.Length }
        }
    };
}
