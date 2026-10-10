#!/usr/bin/env python3
"""Edit a decompiled .vpcf: replace the FIRST C_OP_RenderSprites renderer (the
card frame) with a C_OP_RenderModels renderer pointing at a model, keeping the
rest (the suit-symbol sprite). Also relabels the format header vpcf65 -> vpcf63
so the CSDK 12 resourcecompiler accepts it.

Usage: swap_sprite_for_model.py <in.vpcf> <out.vpcf> <model_resource_path>
"""
import sys

def find_first_renderer_span(text):
    # locate m_Renderers = [ , then the first '{' object, brace-match to its '}'
    ri = text.index("m_Renderers")
    lb = text.index("[", ri)
    ob = text.index("{", lb)
    depth = 0
    i = ob
    while i < len(text):
        c = text[i]
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return ob, i + 1
        i += 1
    raise RuntimeError("unbalanced braces in m_Renderers")

def main():
    src, dst, model = sys.argv[1], sys.argv[2], sys.argv[3]
    text = open(src, encoding="utf-8").read()
    ob, ce = find_first_renderer_span(text)
    first = text[ob:ce]
    if "C_OP_RenderSprites" not in first:
        sys.exit(f"first renderer is not C_OP_RenderSprites:\n{first[:120]}")
    # Full field set mirroring the working soul-orb model renderer
    # (particles/generic/holding_gold_neutral_model.vpcf), so the engine treats
    # it identically to a known-good particle-rendered model.
    model_block = (
        "{\n"
        '\t\t\t\t_class = "C_OP_RenderModels"\n'
        "\t\t\t\tm_ModelList =\n\t\t\t\t[\n"
        "\t\t\t\t\t{\n"
        f'\t\t\t\t\t\tm_model = resource:"{model}"\n'
        "\t\t\t\t\t},\n"
        "\t\t\t\t]\n"
        "\t\t\t\tm_nBodyGroupField = 19\n"
        "\t\t\t\tm_nSubModelField = 13\n"
        "\t\t\t\tm_bOrientZ = true\n"
        "\t\t\t\tm_modelInput =\n\t\t\t\t{\n\t\t\t\t}\n"
        "\t\t\t\tm_bAcceptsDecals = false\n"
        "\t\t\t\tm_bEnableClothSimulation = false\n"
        "\t\t\t}"
    )
    text = text[:ob] + model_block + text[ce:]
    # version-skew bridge: live vpcf65 -> CSDK-compilable vpcf63
    text = text.replace(
        "format:vpcf65:version{48e806a9-c210-4cd8-811a-38b5ef63b195}",
        "format:vpcf63:version{a6e6a69e-52d3-4527-8b9c-ff3bb91aca3e}")
    open(dst, "w", encoding="utf-8").write(text)
    print(f"swapped frame sprite -> model renderer ({model})")
    print(f"relabeled vpcf65 -> vpcf63; wrote {dst} ({len(text)} bytes)")

if __name__ == "__main__":
    main()
