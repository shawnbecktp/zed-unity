"""Run against a published unity-event-lsp on PATH."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

GUID = "a" * 32


class Client:
    def __init__(self):
        self.process = subprocess.Popen(["unity-event-lsp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        self.send("initialize", {}, 1)

    def send(self, method, params, request_id=None):
        message = dict(jsonrpc="2.0", method=method, params=params)
        if request_id is not None:
            message["id"] = request_id
        body = json.dumps(message).encode()
        self.process.stdin.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
        self.process.stdin.flush()
        if request_id is not None:
            length = int(self.process.stdout.readline().split(b":")[1])
            assert self.process.stdout.readline() == b"\r\n"
            return json.loads(self.process.stdout.read(length))["result"]

    def references(self, path, name):
        source = path.read_text()
        uri = path.as_uri()
        self.send("textDocument/didOpen", {"textDocument": {"uri": uri, "text": source, "languageId": "csharp", "version": 1}})
        for line, text in enumerate(source.splitlines()):
            if "void " + name + "(" in text:
                return self.send("textDocument/references", {
                    "textDocument": {"uri": uri}, "position": {"line": line, "character": text.index(name)},
                    "context": {"includeDeclaration": False}}, 2)
        raise ValueError(name)

    def close(self):
        self.send("shutdown", {}, 3)
        self.send("exit", {})
        self.process.stdin.close()
        self.process.stdout.close()
        self.process.wait(timeout=5)


def asset(guid):
    return f"""%YAML 1.1
--- !u!114 &1
MonoBehaviour:
  m_OnClick:
    m_PersistentCalls:
      m_Calls:
      - m_Target: {{fileID: 2}}
        m_MethodName: Submit
--- !u!114 &2
MonoBehaviour:
  m_Script: {{fileID: 11500000, guid: {guid}, type: 3}}
"""


class UnityEventReferences(unittest.TestCase):
    def test_gameobject_owners_for_class_and_method(self):
        client = Client()
        self.addCleanup(client.close)
        with tempfile.TemporaryDirectory() as tmp:
            assets = Path(tmp).resolve() / "Assets"
            assets.mkdir()
            script = assets / "Handler.cs"
            script.write_text("class Handler { public void Submit() {} }")
            Path(str(script) + ".meta").write_text("guid: " + GUID)
            data = asset(GUID).replace("  m_OnClick:", "  m_GameObject: {fileID: 10}\n  m_OnClick:")
            data = data.replace("  m_Script:", "  m_GameObject: {fileID: 20}\n  m_Script:")
            data += "--- !u!1 &10\nGameObject:\n  m_Name: Play Button\n--- !u!1 &20\nGameObject:\n  m_Name: Controller\n"
            for filename in ["Main.unity", "Menu.prefab"]:
                (assets / filename).write_text(data)
            client.references(script, "Submit")
            for identifier, owner in [("Handler", "Controller"), ("Submit", "Play Button")]:
                params = {"textDocument": {"uri": script.as_uri()},
                          "position": {"line": 0, "character": script.read_text().index(identifier)}}
                hits = client.send("textDocument/references", params, 4)
                self.assertEqual(len(hits), 2)
                hover = client.send("textDocument/hover", params, 5)["contents"]["value"]
                for filename in ["Main.unity", "Menu.prefab"]:
                    self.assertIn(f"{owner} — {filename}:", hover)

    def test_target_identity_and_refresh(self):
        client = Client()
        self.addCleanup(client.close)
        with tempfile.TemporaryDirectory() as tmp:
            assets = Path(tmp).resolve() / "Assets"
            assets.mkdir()
            script = assets / "Handler.cs"
            script.write_text("public class Handler { public void Submit() {} }")
            Path(str(script) + ".meta").write_text("guid: " + GUID)
            scene = assets / "Main.unity"
            prefab = assets / "Play.prefab"
            scene.write_text(asset(GUID))
            prefab.write_text(asset(GUID))
            (assets / "Other.prefab").write_text(asset("b" * 32))
            (assets / "OutsideEvent.prefab").write_text(asset(GUID).replace("m_PersistentCalls:", "other:"))
            (assets / "MissingTarget.prefab").write_text(asset(GUID).replace("fileID: 2}", "fileID: 0}"))
            hits = client.references(script, "Submit")
            self.assertCountEqual([hit["uri"] for hit in hits], [scene.as_uri(), prefab.as_uri()])
            for hit in hits:
                self.assertEqual(hit["range"], {"start": {"line": 7, "character": 22},
                                                "end": {"line": 7, "character": 28}})
            prefab.unlink()
            scene.write_text(asset(GUID).replace("Submit", "Changed"))
            self.assertEqual(client.references(script, "Submit"), [])


if __name__ == "__main__":
    unittest.main()
