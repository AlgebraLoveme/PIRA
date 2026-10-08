"""Small protocol inventory fixture, not evidence of native backend compatibility."""
import json
from pathlib import Path


def write_schemas(directory, contract, fault=""):
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    for name, expected in contract["schemas"].items():
        schema = {"type": "object", "properties": {key: {} for key in expected.get("fields", [])}, "definitions": {}}
        for definition, names in expected.get("definitions", {}).items():
            schema["definitions"][definition] = {"type": "object", "properties": {key: {} for key in names}}
        for definition, variants in expected.get("variants", {}).items():
            schema["definitions"][definition] = {"oneOf": [
                {"type": "object", "properties": {**{key: {} for key in names}, "type": {"enum": [tag]}}}
                for tag, names in variants.items()]}
        if "methods" in expected:
            schema.pop("properties")
            schema["oneOf"] = []
            for index, (method, names) in enumerate(expected["methods"].items()):
                if fault == "method" and method == "turn/interrupt":
                    continue
                definition = f"Parameters{index}"
                schema["definitions"][definition] = {"type": "object", "properties": {key: {} for key in names}}
                if fault == "field" and method == "turn/start":
                    del schema["definitions"][definition]["properties"]["sandboxPolicy"]
                if fault == "required" and method == "turn/start":
                    schema["definitions"][definition]["required"] = ["newRequiredField"]
                schema["oneOf"].append({"type": "object", "properties": {
                    "id": {"type": "integer"}, "method": {"enum": [method]},
                    "params": {"$ref": f"#/definitions/{definition}"}}, "required": ["method", "params"]})
        if "signals" in expected:
            schema.pop("properties")
            schema["oneOf"] = [{"type": "object", "properties": {"method": {"enum": [signal]}}, "required": ["method"]}
                               for signal in expected["signals"]]
        path = directory / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(schema))
