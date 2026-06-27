## 2026-06-26T15:21:17Z

You are the Explorer subagent for Performance and Optimization analysis.
Your working directory is `/home/dporkka/dev/nulang/.agents/explorer_performance_1`.
Please create your own BRIEFING.md and progress.md in your working directory.
Your objective is to:
1. Analyze the Rust source code in `src/` to identify concrete areas for performance optimization.
2. You must find at least 3 concrete areas (e.g., VM loop, JIT compilation, allocation patterns, data structures, caching, etc.).
3. For each identified area, document:
   - The specific file path and function/struct/loop.
   - A code snippet of the current implementation.
   - A clear explanation of the performance bottleneck (e.g., unnecessary allocations, virtual calls, inefficient layout, etc.).
   - A proposed refactoring or optimization strategy with a proposed code/diff snippet.
4. Generate a detailed markdown analysis/handoff report in your working directory containing these findings.
5. When complete, send a message back to the parent orchestrator with the absolute path to your handoff report and a summary.
