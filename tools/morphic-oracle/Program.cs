// morphic-oracle: golden-output generator for the morphic Rust decoder.
//
// Dev-time tool. Wraps ValveResourceFormat to produce .png and .meta.json
// siblings for each .vtex_c fixture, which the Rust tests then diff against.
//
// Subcommands:
//   generate --fixtures DIR [--force]
//   extract  --vpk PATH --entry NAME --out DIR
//   survey   --vpk PATH --out CSV

using System.Globalization;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using SteamDatabase.ValvePak;
using ValveResourceFormat;
using ValveResourceFormat.IO;
using ValveResourceFormat.ResourceTypes;

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
                "kv3dump"  => Kv3Dump(args[1..]),
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

    // ---------- helpers ----------

    // ---------- kv3dump ----------
    //
    // Independent validation: load a resource file with VRF and confirm the
    // DATA block parses as binary KV3. Used to check that morphic's
    // uncompressed re-encode is spec-valid (not just self-consistent).
    private static int Kv3Dump(string[] args)
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

    private static int PrintUsage()
    {
        Console.WriteLine("usage:");
        Console.WriteLine("  morphic-oracle generate --fixtures DIR [--force]");
        Console.WriteLine("  morphic-oracle extract  --vpk PATH --entry NAME --out DIR");
        Console.WriteLine("  morphic-oracle survey   --vpk PATH --out CSV");
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
}
