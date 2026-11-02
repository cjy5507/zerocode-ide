"""Every exporter, by the name a person asks for it with."""

from exporters import barchart, jsonl, markdown, tsv

EXPORTERS = {"barchart": barchart.export, "jsonl": jsonl.export, "markdown": markdown.export, "tsv": tsv.export}
