#!/usr/bin/env python3
"""合并多份 tokenizer_trie.json（字典树），用于将来训练分词器。

字典树的每个节点形如 {"e": 0或1, "c": {下一字: 节点}}，
两棵树的合并就是逐节点取并集：e 取或，c 递归合并。

用法：
    python3 tools/merge_trie.py a.json b.json c.json -o merged.json
    python3 tools/merge_trie.py 批次目录1/ 批次目录2/ -o merged.json
        （给目录时会自动找里面的 tokenizer_trie.json）

合并结果中 word_count 是去重后的词数，另外带 merged_from 表示合了几份。
"""

import argparse
import json
import sys
from pathlib import Path


def merge_node(a: dict, b: dict) -> None:
    """把 b 并进 a（就地修改 a）"""
    if b.get("e") == 1:
        a["e"] = 1
    for ch, child_b in (b.get("c") or {}).items():
        child_a = a.setdefault("c", {}).get(ch)
        if child_a is None:
            a["c"][ch] = child_b
        else:
            merge_node(child_a, child_b)


def count_words(node: dict) -> int:
    n = 1 if node.get("e") == 1 else 0
    for child in (node.get("c") or {}).values():
        n += count_words(child)
    return n


def load_trie(path: Path) -> dict:
    data = json.loads(path.read_text(encoding="utf-8"))
    root = data.get("root")
    if not isinstance(root, dict):
        raise ValueError(f"{path} 里没有合法的 root 字段")
    return data


def main() -> int:
    ap = argparse.ArgumentParser(description="合并多份分词器字典树")
    ap.add_argument("inputs", nargs="+", help="trie json 文件或包含它的目录")
    ap.add_argument("-o", "--output", required=True, help="输出文件路径")
    args = ap.parse_args()

    files: list[Path] = []
    for raw in args.inputs:
        p = Path(raw)
        if p.is_dir():
            hit = p / "tokenizer_trie.json"
            if hit.exists():
                files.append(hit)
            else:
                print(f"跳过目录（没找到 tokenizer_trie.json）：{p}", file=sys.stderr)
        elif p.exists():
            files.append(p)
        else:
            print(f"跳过不存在的路径：{p}", file=sys.stderr)

    if not files:
        print("一个有效输入都没有", file=sys.stderr)
        return 1

    merged_root: dict = {"e": 0, "c": {}}
    for f in files:
        try:
            data = load_trie(f)
        except (json.JSONDecodeError, ValueError) as e:
            print(f"跳过坏文件 {f}：{e}", file=sys.stderr)
            continue
        merge_node(merged_root, data["root"])
        print(f"已并入 {f}（声明 {data.get('word_count', '?')} 词）")

    out = {
        "format": "coachsource-tokenizer-trie",
        "version": 1,
        "word_count": count_words(merged_root),
        "merged_from": len(files),
        "root": merged_root,
    }
    Path(args.output).write_text(
        json.dumps(out, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    print(f"\n完成：合并 {len(files)} 份 → {out['word_count']} 个词（去重后）")
    print(f"输出：{args.output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
