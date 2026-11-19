# Particle lab map source generator

This produces `dl_benchmark_lab_0` and `dl_benchmark_lab_16` probe maps from
a locally supplied Source 2 CSDK `volumetric_test.vmap`, saved in keyvalues2
format. It does not include the CSDK room, game assets, or compiled maps. The
generator is adapted from `tools/csdk-native/particle_bench.py` in repository
commit `9dea46c`; emitter positions, effect names and seeded IDs are the same,
with unique names and new map names. The 16-load map places 16 active emitters,
cycling through ten effects. This does not establish how many particles are
visible or whether effects work. Deterministic map IDs do not seed particles.

```sh
python3 tools/deadlock-benchmark/map/generate.py \
  --template /mnt/storage/csdk-native/content/citadel_addons/particle_bench/template/volumetric_test.vmap \
  --output /tmp/deadlock-particle-lab-source
```

Output must not exist. Keep the printed SHA-256 and the template's SHA-256
with each experiment. Compile the generated VMAPs in an isolated, compatible
CSDK stage using the [build procedure](../runtime/README.md), and retain the
compiler log and compiled map hash. Use CSDK 13 rather than the old mixed
stage. Both maps compiled with zero reported compile failures and loaded on
the current game server. A client joined the sixteen-emitter map, but its view
and effects have not been visually checked. Both compile logs reported two
missing editor handle materials. Do not redistribute extracted CSDK or retail content without a
distribution plan.
Old compiled maps are historical probes, not current-build validation.

After a successful compiler run, inspect each fresh output independently:

```sh
python3 tools/deadlock-benchmark/map/verify_compiled.py /path/to/dl_benchmark_lab_0.vpk --count 0
python3 tools/deadlock-benchmark/map/verify_compiled.py /path/to/dl_benchmark_lab_16.vpk --count 16
```

This requires `modlock` on PATH. It checks the compiled world, physics, node
and entity resources, and compares emitters with the generated source. Preserve
and inspect the compiler exit status and log separately, including missing
resources and warnings. A structural pass is not proof the game can load or
render the maps, and effect references alone do not prove their dependencies
are present or visible.
