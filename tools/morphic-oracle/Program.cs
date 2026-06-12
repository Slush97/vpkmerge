// morphic-oracle: golden-output generator for the morphic Rust decoder.
//
// Dev-time tool. Wraps ValveResourceFormat to produce .png and .meta.json
// siblings for each .vtex_c fixture, which the Rust tests then diff against.
//
// Subcommands:
//   generate --fixtures DIR [--force]
//   extract  --vpk PATH --entry NAME --out DIR
//   survey   --vpk PATH --out CSV
//   model    --vpk PATH --entry NAME [--base PATH] --out GLB   (golden glTF)
//   kv3-dump --vpk PATH --entry NAME --block FOURCC --out JSON (M1 KV3 golden)
//   validate --file PATH (strict VRF load of a loose resource file; the gate for
//            the in-place KV3 edits, since morphic's own reader is lenient)

using System.Globalization;
using System.Reflection;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;
using System.Threading;
using SteamDatabase.ValvePak;
using ValveKeyValue;
using ValveResourceFormat;
using ValveResourceFormat.Blocks;
using ValveResourceFormat.CompiledShader;
using ValveResourceFormat.IO;
using ValveResourceFormat.ResourceTypes;
using ValveResourceFormat.ResourceTypes.ModelAnimation;
using ValveResourceFormat.Serialization.KeyValues;

namespace MorphicOracle;

internal static class Program
{
    private static readonly JsonSerializerOptions JsonOpts = new()
    {
        WriteIndented = true,
        DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
    };

    public static int Main(string[] args)
    {
        if (args.Length == 0)
        {
            PrintUsage();
            return 2;
        }

        try
        {
            return args[0] switch
            {
                "generate" => Generate(args[1..]),
                "extract"  => Extract(args[1..]),
                "survey"   => Survey(args[1..]),
                "model"    => ModelExport(args[1..]),
                "kv3-dump" => Kv3Dump(args[1..]),
                "kv3dump"  => Kv3Check(args[1..]),
                "mesh-buffers" => MeshBuffers(args[1..]),
                "model-meta" => ModelMeta(args[1..]),
                "anim-meta" => AnimMeta(args[1..]),
                "material-meta" => MaterialMeta(args[1..]),
                "shader-dump" => ShaderDump(args[1..]),
                "dynexpr"  => DynExpr(args[1..]),
                "validate" => Validate(args[1..]),
                "--help" or "-h" => PrintUsage(),
                _ => Fail($"unknown subcommand: {args[0]}"),
            };
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"error: {ex.GetType().Name}: {ex.Message}");
            return 1;
        }
    }

    // ---------- generate ----------

    private static int Generate(string[] args)
    {
        string? fixturesDir = null;
        var force = false;
        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--fixtures":
                    fixturesDir = args[++i];
                    break;
                case "--force":
                    force = true;
                    break;
                default:
                    return Fail($"generate: unknown flag {args[i]}");
            }
        }
        if (fixturesDir is null)
        {
            return Fail("generate: --fixtures DIR required");
        }
        if (!Directory.Exists(fixturesDir))
        {
            return Fail($"generate: fixtures dir does not exist: {fixturesDir}");
        }

        var vtexFiles = Directory.EnumerateFiles(fixturesDir, "*.vtex_c", SearchOption.AllDirectories)
            .OrderBy(p => p, StringComparer.Ordinal)
            .ToList();

        var made = 0;
        var skipped = 0;
        foreach (var vtexPath in vtexFiles)
        {
            var pngPath = Path.ChangeExtension(vtexPath, ".png");
            var metaPath = Path.ChangeExtension(vtexPath, ".meta.json");
            var srcHash = Sha256Hex(File.ReadAllBytes(vtexPath));

            if (!force && File.Exists(pngPath) && File.Exists(metaPath))
            {
                try
                {
                    using var doc = JsonDocument.Parse(File.ReadAllBytes(metaPath));
                    if (doc.RootElement.TryGetProperty("source_sha256", out var sha)
                        && sha.GetString() == srcHash)
                    {
                        skipped++;
                        continue;
                    }
                }
                catch (JsonException) { /* fall through and regenerate */ }
            }

            GenerateOne(vtexPath, pngPath, metaPath, srcHash);
            made++;
            Console.WriteLine($"generated {Path.GetRelativePath(fixturesDir, vtexPath)}");
        }

        Console.WriteLine($"done: {made} generated, {skipped} up-to-date");
        return 0;
    }

    private static void GenerateOne(string vtexPath, string pngPath, string metaPath, string srcHash)
    {
        using var resource = new Resource();
        resource.Read(vtexPath);
        if (resource.DataBlock is not Texture texture)
        {
            throw new InvalidOperationException($"{vtexPath}: not a texture resource");
        }

        using var bitmap = texture.GenerateBitmap();
        var png = TextureExtract.ToPngImage(bitmap);
        File.WriteAllBytes(pngPath, png);

        // For HDR formats, the .png is tone-mapped and useless for bit-level
        // comparison. Dump the raw RgbaF32 bitmap bytes as a sibling .f32 so
        // the Rust harness can diff in float space. Bitmap dims are already
        // ActualWidth x ActualHeight (VRF crops NonPow2 textures), so the
        // file is exactly Width * Height * 16 bytes, row-major LE.
        var f32Path = Path.ChangeExtension(vtexPath, ".f32");
        if (texture.IsHighDynamicRange && bitmap.ColorType == SkiaSharp.SKColorType.RgbaF32)
        {
            File.WriteAllBytes(f32Path, bitmap.GetPixelSpan().ToArray());
        }
        else if (File.Exists(f32Path))
        {
            // A previous run dumped a .f32 (e.g. format changed during dev).
            // Remove it so the Rust harness doesn't load stale data.
            File.Delete(f32Path);
        }

        var meta = new Meta
        {
            Format = texture.Format.ToString(),
            Width = texture.Width,
            Height = texture.Height,
            ActualWidth = texture.ActualWidth,
            ActualHeight = texture.ActualHeight,
            Depth = texture.Depth,
            MipCount = texture.NumMipLevels,
            Flags = texture.Flags.ToString().Split(", ", StringSplitOptions.RemoveEmptyEntries),
            SourceSha256 = srcHash,
            VrfVersion = typeof(Resource).Assembly.GetName().Version?.ToString() ?? "unknown",
            Tolerance = ToleranceFor(texture.Format),
        };
        File.WriteAllText(metaPath, JsonSerializer.Serialize(meta, JsonOpts) + "\n");
    }

    // ---------- extract ----------

    private static int Extract(string[] args)
    {
        string? vpk = null, entry = null, outDir = null;
        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--vpk":   vpk    = args[++i]; break;
                case "--entry": entry  = args[++i]; break;
                case "--out":   outDir = args[++i]; break;
                default: return Fail($"extract: unknown flag {args[i]}");
            }
        }
        if (vpk is null || entry is null || outDir is null)
        {
            return Fail("extract: --vpk, --entry, and --out are required");
        }

        Directory.CreateDirectory(outDir);
        using var pak = new Package();
        pak.Read(vpk);
        var packageEntry = pak.FindEntry(entry)
            ?? throw new FileNotFoundException($"entry not found in VPK: {entry}");
        pak.ReadEntry(packageEntry, out var data);

        var basename = SanitizeBasename(entry);
        var outPath = Path.Combine(outDir, basename);
        File.WriteAllBytes(outPath, data);
        Console.WriteLine($"extracted {entry} -> {outPath} ({data.Length} bytes)");
        return 0;
    }

    // ---------- survey ----------

    private static int Survey(string[] args)
    {
        string? vpk = null, outCsv = null;
        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--vpk": vpk    = args[++i]; break;
                case "--out": outCsv = args[++i]; break;
                default: return Fail($"survey: unknown flag {args[i]}");
            }
        }
        if (vpk is null || outCsv is null)
        {
            return Fail("survey: --vpk and --out are required");
        }

        using var pak = new Package();
        pak.Read(vpk);
        var entries = pak.Entries;
        List<PackageEntry> vtex = (entries is not null
            && entries.TryGetValue("vtex_c", out var list)
            && list is not null)
            ? list
            : new List<PackageEntry>();

        Console.WriteLine($"surveying {vtex.Count} vtex_c entries in {vpk}");
        Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(outCsv))!);
        using var w = new StreamWriter(outCsv, false, Encoding.UTF8);
        w.WriteLine("entry,format,width,height,mips,flags,kv3_version_hex,bytes");

        var formatCounts = new SortedDictionary<string, int>(StringComparer.Ordinal);
        foreach (var e in vtex)
        {
            try
            {
                pak.ReadEntry(e, out var bytes);
                using var resource = new Resource();
                using var ms = new MemoryStream(bytes);
                resource.Read(ms);
                if (resource.DataBlock is not Texture t)
                {
                    continue;
                }
                var kv3Hex = TryReadKv3Magic(bytes, resource);
                w.WriteLine(string.Join(",",
                    Csv(EntryPath(e)),
                    t.Format,
                    t.Width.ToString(CultureInfo.InvariantCulture),
                    t.Height.ToString(CultureInfo.InvariantCulture),
                    t.NumMipLevels.ToString(CultureInfo.InvariantCulture),
                    Csv(t.Flags.ToString()),
                    kv3Hex,
                    bytes.Length.ToString(CultureInfo.InvariantCulture)));
                var key = t.Format.ToString();
                formatCounts[key] = formatCounts.TryGetValue(key, out var c) ? c + 1 : 1;
            }
            catch (Exception ex)
            {
                w.WriteLine(string.Join(",",
                    Csv(EntryPath(e)), "ERROR", "0", "0", "0", "", "",
                    Csv(ex.GetType().Name)));
            }
        }

        Console.WriteLine("format counts:");
        foreach (var (k, v) in formatCounts)
        {
            Console.WriteLine($"  {k,-22} {v}");
        }
        Console.WriteLine($"wrote {outCsv}");
        return 0;
    }

    // ---------- model (golden .glb) ----------

    // Produces the golden glTF the Rust exporter is diffed against. Mirrors the
    // arg shape of `vpkmerge model export`: --vpk is where the .vmdl_c lives,
    // --base is the package external refs (materials/textures/skeleton) resolve
    // against. For a base hero model the two are the same pak01_dir.vpk.
    private static int ModelExport(string[] args)
    {
        string? vpk = null, entry = null, basePak = null, outPath = null;
        var noMaterials = false;
        var noAnimations = false;
        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--vpk":           vpk          = args[++i]; break;
                case "--entry":         entry        = args[++i]; break;
                case "--base":          basePak      = args[++i]; break;
                case "--out":           outPath      = args[++i]; break;
                case "--no-materials":  noMaterials  = true; break;
                case "--no-animations": noAnimations = true; break;
                default: return Fail($"model: unknown flag {args[i]}");
            }
        }
        if (vpk is null || entry is null || outPath is null)
        {
            return Fail("model: --vpk, --entry, and --out are required");
        }
        basePak ??= vpk;

        using var pak = new Package();
        pak.Read(basePak);
        var loader = new GameFileLoader(pak, basePak);

        // If the model lives in a separate (skin) VPK, search it first so its
        // overrides win over the base pak.
        Package? skinPak = null;
        if (!string.Equals(Path.GetFullPath(vpk), Path.GetFullPath(basePak), StringComparison.Ordinal))
        {
            skinPak = new Package();
            skinPak.Read(vpk);
            loader.AddPackageToSearch(skinPak);
        }

        var sourcePak = skinPak ?? pak;
        var packageEntry = sourcePak.FindEntry(entry)
            ?? throw new FileNotFoundException($"entry not found in VPK: {entry}");
        sourcePak.ReadEntry(packageEntry, out var data);

        using var resource = new Resource();
        using var ms = new MemoryStream(data);
        resource.Read(ms);
        resource.FileName = entry;

        var exporter = new GltfModelExporter(loader)
        {
            ProgressReporter = new Progress<string>(s => Console.Error.WriteLine($"  {s}")),
            ExportMaterials = !noMaterials,
            // Keep animations on so the golden carries the full skeleton + skin
            // (joints, inverse-bind matrices, bone names). morphic only needs to
            // match the skin; it emits no animation samplers, but M3 validates
            // joint count + bone-name set against this golden's skin, so the
            // skeleton must be present. With animations off VRF drops it.
            ExportAnimations = !noAnimations,
        };

        var outFull = Path.GetFullPath(outPath);
        Directory.CreateDirectory(Path.GetDirectoryName(outFull)!);
        exporter.Export(resource, outFull, CancellationToken.None);
        Console.WriteLine($"exported {entry} -> {outFull}");
        skinPak?.Dispose();
        return 0;
    }

    // ---------- kv3-dump (M1 validation) ----------

    // Dumps a single KV3 block (DATA, MDAT, CTRL, AGRP, ...) of a resource as
    // canonical JSON for diffing against morphic's kv3 parser, and optionally
    // writes the raw block bytes (a self-contained KV3 document) for committing
    // as a morphic fixture.
    private static int Kv3Dump(string[] args)
    {
        string? vpk = null, entry = null, blockName = null, outJson = null, rawOut = null;
        var nth = 0;
        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--vpk":   vpk       = args[++i]; break;
                case "--entry": entry     = args[++i]; break;
                case "--block": blockName = args[++i]; break;
                case "--nth":   nth       = int.Parse(args[++i], CultureInfo.InvariantCulture); break;
                case "--out":   outJson   = args[++i]; break;
                case "--raw":   rawOut    = args[++i]; break;
                default: return Fail($"kv3-dump: unknown flag {args[i]}");
            }
        }
        if (vpk is null || entry is null || blockName is null || outJson is null)
        {
            return Fail("kv3-dump: --vpk, --entry, --block, and --out are required");
        }
        if (!Enum.TryParse<BlockType>(blockName, ignoreCase: false, out var blockType))
        {
            return Fail($"kv3-dump: unknown block type {blockName}");
        }

        using var pak = new Package();
        pak.Read(vpk);
        var packageEntry = pak.FindEntry(entry)
            ?? throw new FileNotFoundException($"entry not found in VPK: {entry}");
        pak.ReadEntry(packageEntry, out var data);

        using var resource = new Resource();
        using var ms = new MemoryStream(data);
        resource.Read(ms);

        var matches = resource.Blocks.Where(b => b.Type == blockType).ToList();
        if (matches.Count == 0)
        {
            return Fail($"kv3-dump: no {blockName} block in {entry}");
        }
        if (nth < 0 || nth >= matches.Count)
        {
            return Fail($"kv3-dump: --nth {nth} out of range ({matches.Count} {blockName} blocks)");
        }

        var block = matches[nth];
        // VRF wraps a KV3 block either as a raw BinaryKV3 or, for blocks it
        // recognizes (DATA -> Model, MDAT -> Mesh, ...), as a KeyValuesOrNTRO
        // subclass. Both expose the parsed tree as a KVObject.
        var kvData = block switch
        {
            BinaryKV3 b => b.Data,
            KeyValuesOrNTRO knv => knv.Data,
            _ => null,
        };
        if (kvData is null)
        {
            return Fail($"kv3-dump: {blockName}[{nth}] is not KV3 (got {block.GetType().Name})");
        }

        var json = KvToJson(kvData);
        var outFull = Path.GetFullPath(outJson);
        Directory.CreateDirectory(Path.GetDirectoryName(outFull)!);
        File.WriteAllText(outFull, (json?.ToJsonString(JsonOpts) ?? "null") + "\n");
        Console.WriteLine($"dumped {blockName}[{nth}] of {entry} -> {outFull}");

        if (rawOut is not null)
        {
            // Offset/Size are absolute within the resource file (same convention
            // the survey path already relies on). The slice is a complete KV3
            // document: magic + header + (LZ4) payload.
            var raw = new byte[block.Size];
            Array.Copy(data, (int)block.Offset, raw, 0, (int)block.Size);
            var rawFull = Path.GetFullPath(rawOut);
            Directory.CreateDirectory(Path.GetDirectoryName(rawFull)!);
            File.WriteAllBytes(rawFull, raw);
            Console.WriteLine($"  raw block -> {rawFull} ({raw.Length} bytes)");
        }
        return 0;
    }

    // Canonical JSON encoding of a KV3 tree. The encoding is chosen so morphic's
    // Value tree maps to the exact same JSON, and the Rust comparator can match
    // it without float-formatting ambiguity:
    //   - ints / uints  -> JSON number
    //   - floats (f32 widened, f64) -> {"$f64":"0xHEXBITS"} (IEEE-754 bit pattern)
    //   - binary blobs  -> {"$bin":{"len":N,"sha256":"..."}}
    //   - collections   -> JSON object (compared by key, order-insensitive)
    private static JsonNode? KvToJson(KVObject o)
    {
        switch (o.ValueType)
        {
            case KVValueType.Null:
                return null;
            case KVValueType.Boolean:
                return JsonValue.Create((bool)o);
            case KVValueType.Int16:
            case KVValueType.Int32:
            case KVValueType.Int64:
                return JsonValue.Create((long)o);
            case KVValueType.UInt16:
            case KVValueType.UInt32:
            case KVValueType.UInt64:
                return JsonValue.Create((ulong)o);
            case KVValueType.FloatingPoint:
            case KVValueType.FloatingPoint64:
                return FloatNode((double)o);
            case KVValueType.String:
                return JsonValue.Create((string)o);
            case KVValueType.BinaryBlob:
                return BlobNode((byte[])o);
            case KVValueType.Array:
            {
                var arr = new JsonArray();
                for (var i = 0; i < o.Count; i++)
                {
                    arr.Add(KvToJson(o[i]));
                }
                return arr;
            }
            case KVValueType.Collection:
            {
                var obj = new JsonObject();
                foreach (var key in o.Keys)
                {
                    obj[key] = KvToJson(o[key]);
                }
                return obj;
            }
            default:
                throw new NotSupportedException($"kv3-dump: KV3 value type {o.ValueType} not handled");
        }
    }

    private static JsonObject FloatNode(double d) =>
        new() { ["$f64"] = $"0x{BitConverter.DoubleToUInt64Bits(d):X16}" };

    private static JsonObject BlobNode(byte[] blob) =>
        new() { ["$bin"] = new JsonObject { ["len"] = blob.Length, ["sha256"] = Sha256Hex(blob) } };

    // ---------- mesh-buffers (M2 meshopt validation) ----------

    // For each embedded mesh in a .vmdl_c, reconstructs the VBIB the way VRF
    // does (so MVTX/MIDX blocks get meshopt-decoded) and writes, per buffer:
    //   <mesh>_v{j}.meshopt / _i{j}.meshopt  -> raw compressed block bytes
    //   <mesh>_v{j}.meshopt.json / ...        -> decoded-buffer golden record
    // morphic decodes the raw block and matches the golden's length + SHA-256.
    private static int MeshBuffers(string[] args)
    {
        string? vpk = null, entry = null, outDir = null;
        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--vpk":     vpk    = args[++i]; break;
                case "--entry":   entry  = args[++i]; break;
                case "--out-dir": outDir = args[++i]; break;
                default: return Fail($"mesh-buffers: unknown flag {args[i]}");
            }
        }
        if (vpk is null || entry is null || outDir is null)
        {
            return Fail("mesh-buffers: --vpk, --entry, and --out-dir are required");
        }

        using var pak = new Package();
        pak.Read(vpk);
        var packageEntry = pak.FindEntry(entry)
            ?? throw new FileNotFoundException($"entry not found in VPK: {entry}");
        pak.ReadEntry(packageEntry, out var data);

        using var resource = new Resource { FileName = entry };
        using var ms = new MemoryStream(data);
        resource.Read(ms);

        KVObject? root = resource.GetBlockByType(BlockType.CTRL) switch
        {
            BinaryKV3 b => b.Data.Root,
            KeyValuesOrNTRO k => k.Data,
            _ => null,
        };
        if (root is null)
        {
            return Fail("mesh-buffers: no CTRL block (embedded mesh control)");
        }

        Directory.CreateDirectory(Path.GetFullPath(outDir));
        var records = new List<MeshBufferRecord>();

        foreach (var em in root.GetArray("embedded_meshes"))
        {
            var name = em.GetStringProperty("m_Name");
            var vbib = new VBIB(resource, em) { Resource = resource };

            EmitBuffers(data, resource, outDir, records, name, "vertex",
                em.GetArray("m_vertexBuffers"), vbib.VertexBuffers);
            EmitBuffers(data, resource, outDir, records, name, "index",
                em.GetArray("m_indexBuffers"), vbib.IndexBuffers);
        }

        File.WriteAllText(Path.Combine(Path.GetFullPath(outDir), "manifest.json"),
            JsonSerializer.Serialize(records, JsonOpts) + "\n");
        Console.WriteLine($"wrote {records.Count} mesh buffers -> {outDir}");
        return 0;
    }

    private static void EmitBuffers(
        byte[] data, Resource resource, string outDir, List<MeshBufferRecord> records,
        string mesh, string kind, IReadOnlyList<KVObject> descriptors, List<VBIB.OnDiskBufferData> buffers)
    {
        for (var j = 0; j < buffers.Count; j++)
        {
            var desc = descriptors[j];
            var blockIndex = desc.GetInt32Property("m_nBlockIndex");
            var block = resource.GetBlockByIndex(blockIndex);
            var raw = new byte[block.Size];
            Array.Copy(data, (int)block.Offset, raw, 0, (int)block.Size);

            var prefix = kind == "vertex" ? "v" : "i";
            var baseName = $"{SanitizeBasename(mesh)}_{prefix}{j}";
            File.WriteAllBytes(Path.Combine(Path.GetFullPath(outDir), baseName + ".meshopt"), raw);

            var rec = new MeshBufferRecord
            {
                Mesh = mesh,
                Kind = kind,
                BufferIndex = j,
                BlockIndex = blockIndex,
                ElementCount = buffers[j].ElementCount,
                ElementSize = buffers[j].ElementSizeInBytes,
                Meshopt = desc.GetByteProperty("m_bMeshoptCompressed") == 1,
                Zstd = desc.GetByteProperty("m_bCompressedZSTD") == 1,
                CompressedSize = (int)block.Size,
                DecodedLen = buffers[j].Data.Length,
                Sha256 = Sha256Hex(buffers[j].Data),
            };
            records.Add(rec);
            File.WriteAllText(
                Path.Combine(Path.GetFullPath(outDir), baseName + ".meshopt.json"),
                JsonSerializer.Serialize(rec, JsonOpts) + "\n");
        }
    }

    // ---------- model-meta (M3 validation) ----------

    // Emits a compact, buffer-free-comparable summary of a model's LOD0 geometry
    // and skin: sorted bone names, per-mesh vertex-buffer layouts + draw calls +
    // materials + scene bounds, vertex/index totals, and a source-space position
    // bbox. morphic's `model::decode` reproduces every field; the committed CI
    // test checks the parts derivable from the committed CTRL/DATA/MDAT[0]
    // fixtures, the gated local test checks the whole thing against a real VPK.
    private static int ModelMeta(string[] args)
    {
        string? vpk = null, entry = null, outJson = null;
        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--vpk":   vpk     = args[++i]; break;
                case "--entry": entry   = args[++i]; break;
                case "--out":   outJson = args[++i]; break;
                default: return Fail($"model-meta: unknown flag {args[i]}");
            }
        }
        if (vpk is null || entry is null || outJson is null)
        {
            return Fail("model-meta: --vpk, --entry, and --out are required");
        }

        using var pak = new Package();
        pak.Read(vpk);
        var packageEntry = pak.FindEntry(entry)
            ?? throw new FileNotFoundException($"entry not found in VPK: {entry}");
        pak.ReadEntry(packageEntry, out var data);

        using var resource = new Resource { FileName = entry };
        using var ms = new MemoryStream(data);
        resource.Read(ms);

        if (resource.DataBlock is not Model model)
        {
            return Fail("model-meta: resource is not a model");
        }

        var meta = new ModelMeta_t
        {
            BoneNames = model.Skeleton.Bones
                .Select(b => b.Name)
                .OrderBy(n => n, StringComparer.Ordinal)
                .ToArray(),
            VrfVersion = typeof(Resource).Assembly.GetName().Version?.ToString() ?? "unknown",
        };
        meta.BoneCount = meta.BoneNames.Length;

        var meshes = new List<MeshMeta_t>();
        var bbMin = new[] { float.PositiveInfinity, float.PositiveInfinity, float.PositiveInfinity };
        var bbMax = new[] { float.NegativeInfinity, float.NegativeInfinity, float.NegativeInfinity };
        var materials = new SortedSet<string>(StringComparer.Ordinal);
        var uniqueVerts = 0;
        var gltfVerts = 0;
        var totalIndices = 0;

        foreach (var (mesh, meshIndex, name, lodMask) in model.GetEmbeddedMeshesAndLoD())
        {
            if ((lodMask & 1) == 0)
            {
                continue; // LOD0 only
            }

            var vbib = mesh.VBIB;
            var vbMetas = new List<VbMeta_t>();
            foreach (var vb in vbib.VertexBuffers)
            {
                uniqueVerts += (int)vb.ElementCount;
                vbMetas.Add(new VbMeta_t
                {
                    ElementCount = (int)vb.ElementCount,
                    ElementSize = (int)vb.ElementSizeInBytes,
                    Fields = vb.InputLayoutFields.Select(f => new FieldMeta_t
                    {
                        Semantic = f.SemanticName,
                        SemanticIndex = f.SemanticIndex,
                        Format = (int)f.Format,
                        Offset = (int)f.Offset,
                    }).ToArray(),
                });

                var posField = vb.InputLayoutFields.FirstOrDefault(f => f.SemanticName == "POSITION");
                if (posField.SemanticName == "POSITION")
                {
                    foreach (var p in VBIB.GetVector3AttributeArray(vb, posField))
                    {
                        bbMin[0] = MathF.Min(bbMin[0], p.X);
                        bbMin[1] = MathF.Min(bbMin[1], p.Y);
                        bbMin[2] = MathF.Min(bbMin[2], p.Z);
                        bbMax[0] = MathF.Max(bbMax[0], p.X);
                        bbMax[1] = MathF.Max(bbMax[1], p.Y);
                        bbMax[2] = MathF.Max(bbMax[2], p.Z);
                    }
                }
            }

            var ibMetas = vbib.IndexBuffers.Select(ib => new IbMeta_t
            {
                ElementCount = (int)ib.ElementCount,
                ElementSize = (int)ib.ElementSizeInBytes,
            }).ToArray();

            var prims = new List<PrimMeta_t>();
            var sceneMin = new[] { float.PositiveInfinity, float.PositiveInfinity, float.PositiveInfinity };
            var sceneMax = new[] { float.NegativeInfinity, float.NegativeInfinity, float.NegativeInfinity };

            foreach (var so in mesh.Data.GetArray("m_sceneObjects"))
            {
                AggregateBounds(so, "m_vMinBounds", sceneMin, isMin: true);
                AggregateBounds(so, "m_vMaxBounds", sceneMax, isMin: false);

                foreach (var dc in so.GetArray("m_drawCalls"))
                {
                    var vbIdx = dc.GetArray("m_vertexBuffers")[0].GetInt32Property("m_hBuffer");
                    var indexCount = dc.GetInt32Property("m_nIndexCount");
                    var material = dc.GetStringProperty("m_material");

                    prims.Add(new PrimMeta_t
                    {
                        VertexBuffer = vbIdx,
                        VertexCount = dc.GetInt32Property("m_nVertexCount"),
                        IndexCount = indexCount,
                        Material = material,
                    });

                    gltfVerts += (int)vbib.VertexBuffers[vbIdx].ElementCount;
                    totalIndices += indexCount;
                    materials.Add(material);
                }
            }

            meshes.Add(new MeshMeta_t
            {
                Name = name,
                MeshIndex = meshIndex,
                SceneMin = Finite(sceneMin),
                SceneMax = Finite(sceneMax),
                VertexBuffers = vbMetas.ToArray(),
                IndexBuffers = ibMetas,
                Primitives = prims.ToArray(),
            });
        }

        meta.Meshes = meshes.ToArray();
        meta.UniqueVertices = uniqueVerts;
        meta.GltfVertices = gltfVerts;
        meta.TotalIndices = totalIndices;
        meta.Materials = materials.ToArray();
        meta.MaterialCount = materials.Count;
        meta.BboxMin = Finite(bbMin);
        meta.BboxMax = Finite(bbMax);

        var outFull = Path.GetFullPath(outJson);
        Directory.CreateDirectory(Path.GetDirectoryName(outFull)!);
        File.WriteAllText(outFull, JsonSerializer.Serialize(meta, JsonOpts) + "\n");
        Console.WriteLine($"wrote model meta for {entry} -> {outFull}");
        Console.WriteLine($"  bones={meta.BoneCount} meshes={meta.Meshes.Length} " +
            $"unique_verts={uniqueVerts} gltf_verts={gltfVerts} indices={totalIndices} materials={materials.Count}");
        return 0;
    }

    private static void AggregateBounds(KVObject sceneObject, string key, float[] acc, bool isMin)
    {
        if (!sceneObject.ContainsKey(key))
        {
            return;
        }
        var v = sceneObject.GetSubCollection(key).ToVector3();
        var comps = new[] { v.X, v.Y, v.Z };
        for (var i = 0; i < 3; i++)
        {
            acc[i] = isMin ? MathF.Min(acc[i], comps[i]) : MathF.Max(acc[i], comps[i]);
        }
    }

    private static float[] Finite(float[] v) =>
        float.IsFinite(v[0]) ? v : new float[] { 0f, 0f, 0f };

    // ---------- anim-meta (animation-decode validation) ----------

    // Dumps the model's embedded animation clips (VRF's GetEmbeddedAnimations,
    // the same set GltfModelExporter writes) as the golden the morphic animation
    // decoder diffs against: per-clip name/fps/frame_count/looping, plus a few
    // sampled per-bone keyframe values decoded in raw Source space. The raw
    // ANIM/AGRP/ASEQ blocks are NOT committed (multi-MB); the gated Rust test
    // reads them live from the pak. Small JSON; committed.
    private static int AnimMeta(string[] args)
    {
        string? vpk = null, entry = null, outJson = null;
        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--vpk":   vpk     = args[++i]; break;
                case "--entry": entry   = args[++i]; break;
                case "--out":   outJson = args[++i]; break;
                default: return Fail($"anim-meta: unknown flag {args[i]}");
            }
        }
        if (vpk is null || entry is null || outJson is null)
        {
            return Fail("anim-meta: --vpk, --entry, and --out are required");
        }

        using var pak = new Package();
        pak.Read(vpk);
        var packageEntry = pak.FindEntry(entry)
            ?? throw new FileNotFoundException($"entry not found in VPK: {entry}");
        pak.ReadEntry(packageEntry, out var data);

        using var resource = new Resource { FileName = entry };
        using var ms = new MemoryStream(data);
        resource.Read(ms);

        if (resource.DataBlock is not Model model)
        {
            return Fail("anim-meta: resource is not a model");
        }

        var skeleton = model.Skeleton;
        var flex = model.FlexControllers;
        // The clips morphic emits to the glb: frame_count >= 1, ordered by name.
        var anims = model.GetEmbeddedAnimations()
            .Where(a => a.FrameCount >= 1)
            .OrderBy(a => a.Name, StringComparer.Ordinal)
            .ToList();
        var byName = anims.GroupBy(a => a.Name).ToDictionary(g => g.Key, g => g.First());

        var clips = anims.Select(a => new ClipMeta_t
        {
            Name = a.Name,
            Fps = a.Fps,
            FrameCount = a.FrameCount,
            Looping = a.IsLooping,
        }).ToArray();

        // Probe a spread of (clip, bone, channel) on a real idle + UI/loadout
        // poses, sampling start / middle / last frame, to validate the position,
        // angle (packed-quaternion), and scale decoders end-to-end.
        var probes = new (string Clip, string Bone, string Channel)[]
        {
            ("primary_stand_idle", "pelvis", "rotation"),
            ("primary_stand_idle", "pelvis", "translation"),
            ("primary_stand_idle", "spine_0", "rotation"),
            ("idle_loadout", "pelvis", "rotation"),
            ("ui_hero_select", "pelvis", "rotation"),
        };

        var samples = new List<SampleMeta_t>();
        foreach (var (clipName, boneName, channel) in probes)
        {
            if (!byName.TryGetValue(clipName, out var anim))
            {
                continue;
            }
            var boneIdx = Array.FindIndex(skeleton.Bones, b => b.Name == boneName);
            if (boneIdx < 0)
            {
                continue;
            }
            var last = anim.FrameCount - 1;
            foreach (var frame in new[] { 0, anim.FrameCount / 2, last }.Distinct())
            {
                var f = new Frame(skeleton, flex) { FrameIndex = frame };
                anim.DecodeFrame(f);
                var bone = f.Bones[boneIdx];
                var value = channel switch
                {
                    "translation" => new[] { bone.Position.X, bone.Position.Y, bone.Position.Z },
                    "rotation" => new[] { bone.Angle.X, bone.Angle.Y, bone.Angle.Z, bone.Angle.W },
                    "scale" => new[] { bone.Scale },
                    _ => Array.Empty<float>(),
                };
                samples.Add(new SampleMeta_t
                {
                    Clip = clipName,
                    Bone = boneName,
                    Channel = channel,
                    Frame = frame,
                    Value = value,
                });
            }
        }

        var meta = new AnimMeta_t
        {
            ClipCount = clips.Length,
            Clips = clips,
            Samples = samples.ToArray(),
            VrfVersion = typeof(Resource).Assembly.GetName().Version?.ToString() ?? "unknown",
        };

        var outFull = Path.GetFullPath(outJson);
        Directory.CreateDirectory(Path.GetDirectoryName(outFull)!);
        File.WriteAllText(outFull, JsonSerializer.Serialize(meta, JsonOpts) + "\n");
        Console.WriteLine($"wrote anim meta for {entry} -> {outFull}");
        Console.WriteLine($"  clips={meta.ClipCount} samples={meta.Samples.Length}");
        return 0;
    }

    // ---------- material-meta (M4 validation) ----------

    // Dumps a compiled material's shader name + parameter tables (as VRF's
    // Material parses them) for the morphic material parser to diff against.
    // The .vmat_c itself is committed as a fixture; this is the golden.
    private static int MaterialMeta(string[] args)
    {
        string? vpk = null, entry = null, outJson = null;
        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--vpk":   vpk     = args[++i]; break;
                case "--entry": entry   = args[++i]; break;
                case "--out":   outJson = args[++i]; break;
                default: return Fail($"material-meta: unknown flag {args[i]}");
            }
        }
        if (vpk is null || entry is null || outJson is null)
        {
            return Fail("material-meta: --vpk, --entry, and --out are required");
        }

        using var pak = new Package();
        pak.Read(vpk);
        var packageEntry = pak.FindEntry(entry)
            ?? throw new FileNotFoundException($"entry not found in VPK: {entry}");
        pak.ReadEntry(packageEntry, out var data);

        using var resource = new Resource { FileName = entry };
        using var ms = new MemoryStream(data);
        resource.Read(ms);

        if (resource.DataBlock is not Material mat)
        {
            return Fail("material-meta: resource is not a material");
        }

        var meta = new MaterialMeta_t
        {
            Name = mat.Name,
            ShaderName = mat.ShaderName,
            TextureParams = new SortedDictionary<string, string>(mat.TextureParams, StringComparer.Ordinal),
            IntParams = new SortedDictionary<string, long>(mat.IntParams, StringComparer.Ordinal),
            FloatParams = mat.FloatParams
                .OrderBy(kv => kv.Key, StringComparer.Ordinal)
                .ToDictionary(kv => kv.Key, kv => (double)kv.Value),
            VectorParams = mat.VectorParams
                .OrderBy(kv => kv.Key, StringComparer.Ordinal)
                .ToDictionary(kv => kv.Key, kv => new[] { kv.Value.X, kv.Value.Y, kv.Value.Z, kv.Value.W }),
            VrfVersion = typeof(Resource).Assembly.GetName().Version?.ToString() ?? "unknown",
        };

        var outFull = Path.GetFullPath(outJson);
        Directory.CreateDirectory(Path.GetDirectoryName(outFull)!);
        File.WriteAllText(outFull, JsonSerializer.Serialize(meta, JsonOpts) + "\n");
        Console.WriteLine($"wrote material meta for {entry} -> {outFull}");
        Console.WriteLine($"  shader={meta.ShaderName} textures={meta.TextureParams.Count} " +
            $"int={meta.IntParams.Count} float={meta.FloatParams.Count} vector={meta.VectorParams.Count}");
        return 0;
    }

    // ---------- validate (strict load of a LOOSE resource file) ----------
    //
    // Strict gate for the in-place KV3 edits: VRF parses the whole container and
    // fully materializes the DATA block (for a material, every param), which forces
    // it to read the binary-blob section using the per-frame size table. morphic's
    // own reader is lenient about a stale blob frame table; VRF is not, so this is
    // the load that catches a mis-framed blobbed `.vmat_c` (the failure mode that
    // rendered the covered mesh as a red error material in-game). Reads a loose file
    // path (not a VPK), so a patched fixture can be validated directly.
    //   validate --file PATH
    private static int Validate(string[] args)
    {
        string? file = null;
        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--file": file = args[++i]; break;
                default: return Fail($"validate: unknown flag {args[i]}");
            }
        }
        if (file is null)
        {
            return Fail("validate: --file is required");
        }

        using var resource = new Resource { FileName = Path.GetFileName(file) };
        resource.Read(file);
        var db = resource.DataBlock;
        if (db is Material mat)
        {
            // Touch every param collection so the KV3 (and its blob section) is
            // fully parsed, not lazily deferred.
            var count = mat.TextureParams.Count + mat.IntParams.Count
                + mat.FloatParams.Count + mat.VectorParams.Count;
            Console.WriteLine($"OK material '{mat.Name}' shader={mat.ShaderName} params={count}");
            foreach (var kv in mat.VectorParams.OrderBy(kv => kv.Key, StringComparer.Ordinal))
            {
                Console.WriteLine($"  vec {kv.Key} = ({kv.Value.X}, {kv.Value.Y}, {kv.Value.Z}, {kv.Value.W})");
            }
            foreach (var kv in mat.TextureParams.OrderBy(kv => kv.Key, StringComparer.Ordinal))
            {
                Console.WriteLine($"  tex {kv.Key} = {kv.Value}");
            }
        }
        else
        {
            Console.WriteLine($"OK resource, DATA block = {db?.GetType().Name ?? "none"}");
        }
        return 0;
    }

    // ---------- helpers ----------

    // ---------- kv3dump (soundevents re-encode validation) ----------
    //
    // Independent validation: load a resource file with VRF and confirm the
    // DATA block parses as binary KV3. Used to check that morphic's
    // uncompressed re-encode is spec-valid (not just self-consistent). Distinct
    // from `kv3-dump` (the block-by-FOURCC golden dumper above).
    private static int Kv3Check(string[] args)
    {
        string? file = null;
        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--file": file = args[++i]; break;
                default: return Fail($"kv3dump: unknown flag {args[i]}");
            }
        }
        if (file is null)
        {
            return Fail("kv3dump: --file is required");
        }

        using var resource = new Resource();
        resource.Read(file);

        var blockTypes = new List<string>();
        foreach (var b in resource.Blocks)
        {
            blockTypes.Add(b.Type.ToString());
        }
        Console.Error.WriteLine($"loaded {file}: blocks=[{string.Join(",", blockTypes)}]");

        var data = resource.DataBlock;
        if (data is BinaryKV3 kv3)
        {
            Console.Error.WriteLine("OK: DataBlock parsed as BinaryKV3");
            Console.WriteLine(kv3.ToString());
            return 0;
        }

        Console.Error.WriteLine($"FAIL: DataBlock is {data?.GetType().Name ?? "null"}, not BinaryKV3");
        return 1;
    }

    private static string TryReadKv3Magic(byte[] resourceBytes, Resource resource)
    {
        // DATA block magic is KV3 if it starts with 'VKV\x03' or one of the
        // later magic bytes. Read the first 4 bytes of the DATA block.
        var data = resource.DataBlock;
        if (data is null) return "";
        var ofs = data.Offset;
        if (ofs + 4 > (ulong)resourceBytes.LongLength) return "";
        var magic = BitConverter.ToUInt32(resourceBytes, (int)ofs);
        return $"0x{magic:X8}";
    }

    private static string EntryPath(PackageEntry e)
    {
        return string.IsNullOrEmpty(e.DirectoryName)
            ? $"{e.FileName}.{e.TypeName}"
            : $"{e.DirectoryName}/{e.FileName}.{e.TypeName}";
    }

    private static string Csv(string s)
    {
        if (s.Contains(',') || s.Contains('"') || s.Contains('\n'))
        {
            return $"\"{s.Replace("\"", "\"\"")}\"";
        }
        return s;
    }

    private static string SanitizeBasename(string entry)
    {
        var name = Path.GetFileName(entry);
        return string.IsNullOrEmpty(name) ? "extracted.vtex_c" : name;
    }

    private static string Sha256Hex(byte[] data)
    {
        var hash = SHA256.HashData(data);
        var sb = new StringBuilder(hash.Length * 2);
        foreach (var b in hash) sb.Append(b.ToString("x2", CultureInfo.InvariantCulture));
        return sb.ToString();
    }

    private static Tolerance ToleranceFor(VTexFormat fmt)
    {
        // Default tolerances per format family. Hand-edit a fixture's meta to
        // override if a specific decoder needs tighter or looser bounds.
        return fmt switch
        {
            VTexFormat.RGBA8888 => new Tolerance { Kind = "byte_exact" },
            VTexFormat.RGBA16161616F => new Tolerance { Kind = "hdr_eps", Abs = 0.000977, Rel = 0.005 },
            VTexFormat.BC6H => new Tolerance { Kind = "hdr_eps", Abs = 0.000977, Rel = 0.005 },
            VTexFormat.BC7 => new Tolerance { Kind = "mae_u8", Epsilon = 3.0 },
            _ => new Tolerance { Kind = "mae_u8", Epsilon = 2.0 },
        };
    }

    // Dumps a compiled shader's (.vcs) material-parameter vocabulary: every
    // VfxVariableDescription (name + type + UI group + default string) plus the
    // static-combo feature flags (F_*). The features file is the authoritative
    // list of what a .vmat_c can set -- so this answers "does pbr.vfx expose a
    // world-aligned/triplanar/screen-space texcoord knob?" definitively.
    //   shader-dump --vpk shaders_pc_dir.vpk --list [--filter pbr]
    //   shader-dump --vpk shaders_pc_dir.vpk --entry shaders/vfx/pbr_pc_50_features.vcs
    private static int ShaderDump(string[] args)
    {
        string? vpk = null, entry = null, filter = null;
        bool list = false;
        for (int i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--vpk":    vpk = args[++i]; break;
                case "--entry":  entry = args[++i]; break;
                case "--filter": filter = args[++i]; break;
                case "--list":   list = true; break;
                default: return Fail($"shader-dump: unknown arg {args[i]}");
            }
        }
        if (vpk is null) return Fail("shader-dump: --vpk required");

        using var pak = new Package();
        pak.Read(vpk);

        if (list || entry is null)
        {
            var vlist = pak.Entries.TryGetValue("vcs", out var l) ? l : new List<PackageEntry>();
            foreach (var p in vlist.Select(e => e.GetFullPath())
                                   .Where(p => filter is null || p.Contains(filter, StringComparison.OrdinalIgnoreCase))
                                   .OrderBy(p => p, StringComparer.Ordinal))
                Console.WriteLine(p);
            return 0;
        }

        var pe = pak.FindEntry(entry) ?? throw new FileNotFoundException(entry);
        pak.ReadEntry(pe, out var data);

        object program;
        try
        {
            program = Activator.CreateInstance(typeof(VfxProgramData), nonPublic: true)!;
            var read = typeof(VfxProgramData).GetMethod("Read", new[] { typeof(string), typeof(Stream) });
            if (read is null) throw new MissingMethodException("VfxProgramData.Read(string,Stream)");
            using var ms = new MemoryStream(data);
            read.Invoke(program, new object[] { entry, ms });
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"parse failed: {ex.GetBaseException().Message}");
            Console.Error.WriteLine("VfxProgramData ctors:");
            foreach (var c in typeof(VfxProgramData).GetConstructors(BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance))
                Console.Error.WriteLine("  " + c);
            Console.Error.WriteLine("VfxProgramData Read/Unserialize methods:");
            foreach (var m in typeof(VfxProgramData).GetMethods(BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance | BindingFlags.Static)
                              .Where(m => m.Name.Contains("Read") || m.Name.Contains("Unserialize")))
                Console.Error.WriteLine("  " + m);
            return 2;
        }

        var pdata = (VfxProgramData)program;
        var vars = pdata.VariableDescriptions.ToList();
        Console.WriteLine($"# shader {entry}");
        Console.WriteLine($"# {vars.Count} variable descriptions (material params):");
        foreach (var v in vars.OrderBy(v => v.Name, StringComparer.Ordinal))
            Console.WriteLine($"  {v.Name,-44} vfxType={v.VfxType} src={v.VariableSource} ui=[{v.UiGroup}] str=\"{Trunc(v.StringData, 56)}\"");

        var combos = pdata.StaticComboArray.ToList();
        Console.WriteLine($"# {combos.Count} static combos (feature flags):");
        foreach (var c in combos.OrderBy(c => c.Name, StringComparer.Ordinal))
            Console.WriteLine($"  [SF] {c.Name}");
        return 0;
    }

    private static string Trunc(string? s, int n) =>
        string.IsNullOrEmpty(s) ? "" : (s.Length <= n ? s : s[..n] + "...");

    // dynexpr hash NAME...                              VRF StringToken (murmur2) of each name
    // dynexpr brute --hashes HEX[,HEX...]               reverse hashes against stdin wordlist
    // dynexpr decompile --vpk PAK --entry MATERIAL      decompile dynamic params via VfxEval
    private static int DynExpr(string[] args)
    {
        if (args.Length == 0)
        {
            return Fail("dynexpr: expected hash|brute|decompile");
        }

        switch (args[0])
        {
            case "hash":
            {
                foreach (var name in args[1..])
                {
                    var token = ValveResourceFormat.Utils.StringToken.Get(name.ToLowerInvariant());
                    Console.WriteLine($"{token:x8}  {name}");
                }
                return 0;
            }
            case "brute":
            {
                var hexes = args.Length > 2 && args[1] == "--hashes" ? args[2] : null;
                if (hexes is null)
                {
                    return Fail("dynexpr brute: --hashes HEX[,HEX...] required (wordlist on stdin)");
                }
                var targets = hexes.Split(',')
                    .Select(h => uint.Parse(h, NumberStyles.HexNumber, CultureInfo.InvariantCulture))
                    .ToHashSet();
                var found = 0;
                string? line;
                var seen = new HashSet<string>(StringComparer.Ordinal);
                while ((line = Console.In.ReadLine()) != null)
                {
                    var s = line.Trim();
                    if (s.Length == 0 || s.Length > 80 || !seen.Add(s))
                    {
                        continue;
                    }
                    foreach (var cand in new[] { s, s.TrimStart('$'), "$" + s.TrimStart('$') })
                    {
                        var token = ValveResourceFormat.Utils.StringToken.Get(cand.ToLowerInvariant());
                        if (targets.Contains(token))
                        {
                            Console.WriteLine($"MATCH {token:x8}  \"{cand}\" (from \"{s}\")");
                            found++;
                        }
                    }
                }
                Console.Error.WriteLine($"{found} matches");
                return 0;
            }
            case "decompile":
            {
                string? vpk = null, entry = null;
                for (var i = 1; i < args.Length; i++)
                {
                    switch (args[i])
                    {
                        case "--vpk":   vpk   = args[++i]; break;
                        case "--entry": entry = args[++i]; break;
                        default: return Fail($"dynexpr decompile: unknown flag {args[i]}");
                    }
                }
                if (vpk is null || entry is null)
                {
                    return Fail("dynexpr decompile: --vpk and --entry are required");
                }

                using var pak = new Package();
                pak.Read(vpk);
                var packageEntry = pak.FindEntry(entry)
                    ?? throw new FileNotFoundException($"entry not found in VPK: {entry}");
                pak.ReadEntry(packageEntry, out var data);

                using var resource = new Resource { FileName = entry };
                using var ms = new MemoryStream(data);
                resource.Read(ms);

                if (resource.DataBlock is not Material mat)
                {
                    return Fail("dynexpr decompile: resource is not a material");
                }

                Console.WriteLine($"{entry}  [{mat.ShaderName}]");
                foreach (var table in new[] { "m_dynamicParams", "m_dynamicTextureParams" })
                {
                    foreach (var param in mat.Data.GetArray(table) ?? [])
                    {
                        var name = param.GetStringProperty("m_name");
                        var code = param.GetArray<byte>("m_value");
                        var expr = new ValveResourceFormat.Serialization.VfxEval.VfxEval(code).DynamicExpressionResult;
                        Console.WriteLine($"  {name} = {expr}");
                    }
                }
                return 0;
            }
            default:
                return Fail($"dynexpr: unknown mode {args[0]}");
        }
    }

    private static int PrintUsage()
    {
        Console.WriteLine("usage:");
        Console.WriteLine("  morphic-oracle generate --fixtures DIR [--force]");
        Console.WriteLine("  morphic-oracle extract  --vpk PATH --entry NAME --out DIR");
        Console.WriteLine("  morphic-oracle survey   --vpk PATH --out CSV");
        Console.WriteLine("  morphic-oracle model    --vpk PATH --entry NAME [--base PATH] --out GLB [--no-materials]");
        Console.WriteLine("  morphic-oracle kv3-dump --vpk PATH --entry NAME --block FOURCC [--nth N] --out JSON [--raw KV3BIN]");
        Console.WriteLine("  morphic-oracle kv3dump  --file FILE  (validate a re-encoded KV3 file parses)");
        Console.WriteLine("  morphic-oracle mesh-buffers --vpk PATH --entry NAME --out-dir DIR");
        Console.WriteLine("  morphic-oracle model-meta --vpk PATH --entry NAME --out JSON");
        Console.WriteLine("  morphic-oracle anim-meta --vpk PATH --entry NAME --out JSON");
        Console.WriteLine("  morphic-oracle material-meta --vpk PATH --entry NAME --out JSON");
        Console.WriteLine("  morphic-oracle shader-dump --vpk PATH (--list [--filter STR] | --entry VCS)");
        Console.WriteLine("  morphic-oracle validate --file PATH  (strict VRF load of a loose resource file)");
        return 0;
    }

    private static int Fail(string msg)
    {
        Console.Error.WriteLine($"error: {msg}");
        return 2;
    }

    // ---------- JSON shapes ----------

    private sealed class Meta
    {
        [JsonPropertyName("format")]         public string Format { get; init; } = "";
        [JsonPropertyName("width")]          public ushort Width { get; init; }
        [JsonPropertyName("height")]         public ushort Height { get; init; }
        // VRF crops the rendered PNG to ActualWidth/Height when the texture
        // carries NonPow2 metadata; the Rust test crops morphic's full decode
        // to these dims before diffing against the PNG.
        [JsonPropertyName("actual_width")]   public ushort ActualWidth { get; init; }
        [JsonPropertyName("actual_height")]  public ushort ActualHeight { get; init; }
        [JsonPropertyName("depth")]          public ushort Depth { get; init; }
        [JsonPropertyName("mip_count")]      public byte MipCount { get; init; }
        [JsonPropertyName("flags")]          public string[] Flags { get; init; } = Array.Empty<string>();
        [JsonPropertyName("source_sha256")]  public string SourceSha256 { get; init; } = "";
        [JsonPropertyName("vrf_version")]    public string VrfVersion { get; init; } = "";
        [JsonPropertyName("tolerance")]      public Tolerance Tolerance { get; init; } = new();
    }

    private sealed class Tolerance
    {
        [JsonPropertyName("kind")]    public string Kind { get; init; } = "byte_exact";
        [JsonPropertyName("epsilon")] public double? Epsilon { get; init; }
        [JsonPropertyName("abs")]     public double? Abs { get; init; }
        [JsonPropertyName("rel")]     public double? Rel { get; init; }
    }

    private sealed class MeshBufferRecord
    {
        [JsonPropertyName("mesh")]            public string Mesh { get; init; } = "";
        [JsonPropertyName("kind")]            public string Kind { get; init; } = "";
        [JsonPropertyName("buffer_index")]    public int BufferIndex { get; init; }
        [JsonPropertyName("block_index")]     public int BlockIndex { get; init; }
        [JsonPropertyName("element_count")]   public uint ElementCount { get; init; }
        [JsonPropertyName("element_size")]    public uint ElementSize { get; init; }
        [JsonPropertyName("meshopt")]         public bool Meshopt { get; init; }
        [JsonPropertyName("zstd")]            public bool Zstd { get; init; }
        [JsonPropertyName("compressed_size")] public int CompressedSize { get; init; }
        [JsonPropertyName("decoded_len")]     public int DecodedLen { get; init; }
        [JsonPropertyName("sha256")]          public string Sha256 { get; init; } = "";
    }

    private sealed class ModelMeta_t
    {
        [JsonPropertyName("bone_count")]      public int BoneCount { get; set; }
        [JsonPropertyName("bone_names")]      public string[] BoneNames { get; set; } = Array.Empty<string>();
        [JsonPropertyName("meshes")]          public MeshMeta_t[] Meshes { get; set; } = Array.Empty<MeshMeta_t>();
        [JsonPropertyName("unique_vertices")] public int UniqueVertices { get; set; }
        [JsonPropertyName("gltf_vertices")]   public int GltfVertices { get; set; }
        [JsonPropertyName("total_indices")]   public int TotalIndices { get; set; }
        [JsonPropertyName("material_count")]  public int MaterialCount { get; set; }
        [JsonPropertyName("materials")]       public string[] Materials { get; set; } = Array.Empty<string>();
        [JsonPropertyName("bbox_min")]        public float[] BboxMin { get; set; } = Array.Empty<float>();
        [JsonPropertyName("bbox_max")]        public float[] BboxMax { get; set; } = Array.Empty<float>();
        [JsonPropertyName("vrf_version")]     public string VrfVersion { get; set; } = "";
    }

    private sealed class MeshMeta_t
    {
        [JsonPropertyName("name")]           public string Name { get; init; } = "";
        [JsonPropertyName("mesh_index")]     public int MeshIndex { get; init; }
        [JsonPropertyName("scene_min")]      public float[] SceneMin { get; init; } = Array.Empty<float>();
        [JsonPropertyName("scene_max")]      public float[] SceneMax { get; init; } = Array.Empty<float>();
        [JsonPropertyName("vertex_buffers")] public VbMeta_t[] VertexBuffers { get; init; } = Array.Empty<VbMeta_t>();
        [JsonPropertyName("index_buffers")]  public IbMeta_t[] IndexBuffers { get; init; } = Array.Empty<IbMeta_t>();
        [JsonPropertyName("primitives")]     public PrimMeta_t[] Primitives { get; init; } = Array.Empty<PrimMeta_t>();
    }

    private sealed class VbMeta_t
    {
        [JsonPropertyName("element_count")] public int ElementCount { get; init; }
        [JsonPropertyName("element_size")]  public int ElementSize { get; init; }
        [JsonPropertyName("fields")]        public FieldMeta_t[] Fields { get; init; } = Array.Empty<FieldMeta_t>();
    }

    private sealed class IbMeta_t
    {
        [JsonPropertyName("element_count")] public int ElementCount { get; init; }
        [JsonPropertyName("element_size")]  public int ElementSize { get; init; }
    }

    private sealed class FieldMeta_t
    {
        [JsonPropertyName("semantic")]       public string Semantic { get; init; } = "";
        [JsonPropertyName("semantic_index")] public int SemanticIndex { get; init; }
        [JsonPropertyName("format")]         public int Format { get; init; }
        [JsonPropertyName("offset")]         public int Offset { get; init; }
    }

    private sealed class PrimMeta_t
    {
        [JsonPropertyName("vertex_buffer")] public int VertexBuffer { get; init; }
        [JsonPropertyName("vertex_count")]  public int VertexCount { get; init; }
        [JsonPropertyName("index_count")]   public int IndexCount { get; init; }
        [JsonPropertyName("material")]      public string Material { get; init; } = "";
    }

    private sealed class MaterialMeta_t
    {
        [JsonPropertyName("name")]           public string Name { get; init; } = "";
        [JsonPropertyName("shader_name")]    public string ShaderName { get; init; } = "";
        [JsonPropertyName("texture_params")] public IDictionary<string, string> TextureParams { get; init; } = new SortedDictionary<string, string>();
        [JsonPropertyName("int_params")]     public IDictionary<string, long> IntParams { get; init; } = new SortedDictionary<string, long>();
        [JsonPropertyName("float_params")]   public IDictionary<string, double> FloatParams { get; init; } = new Dictionary<string, double>();
        [JsonPropertyName("vector_params")]  public IDictionary<string, float[]> VectorParams { get; init; } = new Dictionary<string, float[]>();
        [JsonPropertyName("vrf_version")]    public string VrfVersion { get; init; } = "";
    }

    private sealed class AnimMeta_t
    {
        [JsonPropertyName("clip_count")]  public int ClipCount { get; init; }
        [JsonPropertyName("clips")]       public ClipMeta_t[] Clips { get; init; } = Array.Empty<ClipMeta_t>();
        [JsonPropertyName("samples")]     public SampleMeta_t[] Samples { get; init; } = Array.Empty<SampleMeta_t>();
        [JsonPropertyName("vrf_version")] public string VrfVersion { get; init; } = "";
    }

    private sealed class ClipMeta_t
    {
        [JsonPropertyName("name")]        public string Name { get; init; } = "";
        [JsonPropertyName("fps")]         public float Fps { get; init; }
        [JsonPropertyName("frame_count")] public int FrameCount { get; init; }
        [JsonPropertyName("looping")]     public bool Looping { get; init; }
    }

    private sealed class SampleMeta_t
    {
        [JsonPropertyName("clip")]    public string Clip { get; init; } = "";
        [JsonPropertyName("bone")]    public string Bone { get; init; } = "";
        [JsonPropertyName("channel")] public string Channel { get; init; } = "";
        [JsonPropertyName("frame")]   public int Frame { get; init; }
        [JsonPropertyName("value")]   public float[] Value { get; init; } = Array.Empty<float>();
    }
}
