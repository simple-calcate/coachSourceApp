#!/usr/bin/env python3
"""
合并多份导出的 jsonl，去重并做基本校验。

用途：手机导出一份、电脑导出一份，用这个脚本合成一份干净的训练文件。

用法：
    python3 tools/merge_jsonl.py 手机.jsonl 电脑.jsonl -o merged.jsonl
    python3 tools/merge_jsonl.py *.jsonl -o merged.jsonl --stats-only

去重规则：把一段对话里所有 (角色, 正文) 拼起来算指纹，完全一样只保留一条。
"""

import argparse
import glob
import hashlib
import json
import os
import sys


def fingerprint(obj):
    """对一条记录生成指纹：SFT 用 conversations，预训练用 text。"""
    if "conversations" in obj:
        parts = []
        for t in obj["conversations"]:
            parts.append(t.get("role", ""))
            parts.append("\x1f")
            parts.append(t.get("content", ""))
            parts.append("\x1e")
        raw = "".join(parts)
    elif "text" in obj:
        raw = obj["text"]
    else:
        raw = json.dumps(obj, sort_keys=True, ensure_ascii=False)
    return hashlib.sha256(raw.encode("utf-8")).hexdigest()


def load_lines(path):
    """读一个 jsonl 文件，返回 (有效记录, 坏行数)"""
    good, bad = [], 0
    with open(path, "r", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                obj = json.loads(line)
            except json.JSONDecodeError:
                bad += 1
                continue
            if isinstance(obj, dict):
                good.append(obj)
            else:
                bad += 1
    return good, bad


def check_warnings(objs):
    """给出一些会直接影响训练效果的检查提示"""
    warns = []
    no_assistant = 0
    single_turn = 0
    empty = 0
    for o in objs:
        convs = o.get("conversations")
        if convs is None:
            continue
        roles = [t.get("role") for t in convs]
        if "assistant" not in roles:
            no_assistant += 1
        if len(convs) < 2:
            single_turn += 1
        if any(not (t.get("content") or "").strip() for t in convs):
            empty += 1
    if no_assistant:
        warns.append(
            f"{no_assistant} 条没有 assistant 回复 —— SFT 训练时这条学不到任何东西，建议检查"
        )
    if single_turn:
        warns.append(f"{single_turn} 条只有一轮，确认是不是漏录了")
    if empty:
        warns.append(f"{empty} 条里存在空内容")
    return warns


def main():
    ap = argparse.ArgumentParser(description="合并并去重 jsonl 训练数据")
    ap.add_argument("inputs", nargs="+", help="输入文件，支持通配符")
    ap.add_argument("-o", "--output", help="输出文件；不填则只打印统计")
    ap.add_argument("--stats-only", action="store_true", help="只统计不写文件")
    args = ap.parse_args()

    paths = []
    for p in args.inputs:
        matched = sorted(glob.glob(p))
        paths.extend(matched if matched else [p])

    paths = [p for p in paths if os.path.exists(p)]
    if not paths:
        print("没有找到任何输入文件", file=sys.stderr)
        sys.exit(1)

    seen = set()
    merged = []
    total = 0
    bad = 0

    for p in paths:
        objs, b = load_lines(p)
        bad += b
        total += len(objs)
        added = 0
        for o in objs:
            fp = fingerprint(o)
            if fp in seen:
                continue
            seen.add(fp)
            merged.append(o)
            added += 1
        print(f"  {p}: 读入 {len(objs)} 条，新增 {added} 条")

    print(f"\n合计读入 {total} 条，去重后 {len(merged)} 条，重复 {total - len(merged)} 条")
    if bad:
        print(f"警告：{bad} 行不是合法 JSON，已跳过")

    for w in check_warnings(merged):
        print(f"提示：{w}")

    if args.output and not args.stats_only:
        os.makedirs(os.path.dirname(os.path.abspath(args.output)), exist_ok=True)
        with open(args.output, "w", encoding="utf-8") as f:
            for o in merged:
                f.write(json.dumps(o, ensure_ascii=False) + "\n")
        size = os.path.getsize(args.output)
        print(f"\n已写出 {args.output}（{len(merged)} 条，{size / 1024:.1f} KB）")


if __name__ == "__main__":
    main()
