# Benchmark baselines

`baseline.json` is the tracked reference every `cargo bench` run is compared
against. It records, per benchmark, Criterion's mean point estimate in
nanoseconds plus the per-stage timings taken from `RenderStats::stage_timings`
(Doc 01 §3 order: encode, resolve, flatten, bin, strips, fine, compose).

A run slower than the baseline by more than `OTF_BENCH_THRESHOLD` (default
1.05, i.e. 5%) exits non-zero. Per `AGENTS.md`, raising the threshold to make a
run pass is not an option. Re-record the baseline only when the change is
intended:

```
OTF_BLESS_BENCH=1 cargo bench
```

and explain in the commit message why the new numbers are correct.
