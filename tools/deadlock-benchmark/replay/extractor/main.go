// replay-extractor preserves observed replay events; it does not simulate a match.
package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"

	"github.com/paralin/s2replay"
	"github.com/paralin/s2replay/analysis"
)

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func run() error {
	mode := flag.String("mode", "scan", "scan hero damage or extract a clip")
	demo := flag.String("demo", "", "source .dem")
	out := flag.String("out", "", "new output directory")
	start := flag.Uint("start", 0, "inclusive clip start tick, including lead-in")
	end := flag.Uint("end", 0, "inclusive clip end tick")
	flag.Parse()
	if *demo == "" || *out == "" || (*mode != "scan" && *mode != "clip") ||
		(*mode == "clip" && (*start == 0 || *end <= *start || uint64(*end) > uint64(^uint32(0)))) {
		return errors.New("use -mode scan|clip -demo FILE -out NEW_DIRECTORY [-start TICK -end TICK]")
	}
	data, err := os.ReadFile(*demo)
	if err != nil {
		return err
	}
	p, err := s2replay.NewParser(data)
	if err != nil {
		return err
	}
	if err = os.Mkdir(*out, 0755); err != nil {
		return err
	}
	output, err := os.Create(filepath.Join(*out, "events.jsonl"))
	if err != nil {
		return err
	}
	defer output.Close()
	enc := json.NewEncoder(output)
	counts := map[string]int{}
	heroes := map[int32]s2replay.EntitySample{}
	finalHeroes := map[int32]s2replay.EntitySample{}
	var lastTick uint32
	for {
		e, nextErr := p.NextEvent()
		if errors.Is(nextErr, io.EOF) {
			break
		}
		if nextErr != nil {
			return fmt.Errorf("tick %d: %w", lastTick, nextErr)
		}
		lastTick = e.Tick
		counts[string(e.Type)]++
		if e.Type == s2replay.EventEntitySample && e.EntitySample != nil {
			s := *e.EntitySample
			if s.HasHeroID && s.HeroID != 0 && e.PlayerSlot >= 0 {
				s.PlayerSlot = e.PlayerSlot
				heroes[e.Entity] = s
			}
		}
		if *mode == "scan" {
			if e.Damage == nil {
				continue
			}
			a, aok := heroes[e.Damage.Attacker]
			v, vok := heroes[e.Damage.Victim]
			// Validate that cached hero handles still refer to current entities.
			ac, acok := p.SampleEntity(p.FindEntity(e.Damage.Attacker))
			vc, vcok := p.SampleEntity(p.FindEntity(e.Damage.Victim))
			if !aok || !vok || !a.HasTeam || !v.HasTeam || a.Team == v.Team ||
				!acok || !vcok || ac.EntitySerial != a.EntitySerial || vc.EntitySerial != v.EntitySerial ||
				!a.HasPosition || !v.HasPosition || e.Tick < a.Tick || e.Tick < v.Tick ||
				float64(e.Tick-a.Tick)*p.Clock().TickInterval() > 1 || float64(e.Tick-v.Tick)*p.Clock().TickInterval() > 1 {
				continue
			}
			row := map[string]any{"tick": e.Tick, "game_time": e.GameTime, "damage": e.Damage.Damage,
				"attacker_slot": a.PlayerSlot, "victim_slot": v.PlayerSlot,
				"attacker_hero": a.HeroID, "victim_hero": v.HeroID,
				"attacker_position": [3]float32{a.PositionX, a.PositionY, a.PositionZ},
				"victim_position":   [3]float32{v.PositionX, v.PositionY, v.PositionZ}}
			if err = enc.Encode(row); err != nil {
				return err
			}
		} else {
			if e.Tick > uint32(*end) {
				break
			}
			if e.Tick >= uint32(*start) {
				if err = enc.Encode(e); err != nil {
					return err
				}
			}
			if e.Tick == uint32(*end) {
				for handle, known := range heroes {
					current, ok := p.SampleEntity(p.FindEntity(handle))
					if ok && current.EntitySerial == known.EntitySerial {
						current.PlayerSlot = known.PlayerSlot
						finalHeroes[known.PlayerSlot] = current
					}
				}
			}
		}
	}
	if err = output.Close(); err != nil {
		return err
	}
	sum := sha256.Sum256(data)
	abs, err := filepath.Abs(*demo)
	if err != nil {
		return err
	}
	revision, verified := s2replay.BuildRevision()
	meta := map[string]any{"schema_version": 1, "mode": *mode, "source": abs,
		"parser_revision": revision, "parser_identity_verified": verified,
		"parser_source_digest": s2replay.ParserSourceDigest,
		"source_sha256":        hex.EncodeToString(sum[:]), "last_tick": lastTick,
		"tick_interval_seconds": p.Clock().TickInterval(), "tick_interval_known": p.Clock().TickIntervalKnown(),
		"counts": counts, "start_tick": *start, "end_tick": *end}
	if *mode == "clip" {
		if err = writeJSON(filepath.Join(*out, "final-state.json"), map[string]any{
			"tick": *end, "heroes": finalHeroes,
		}); err != nil {
			return err
		}
		facts, factsErr := analysis.ExtractRunbackFacts(data, analysis.RunbackRequest{Tick: uint32(*start)})
		if factsErr != nil {
			return factsErr
		}
		if err = writeJSON(filepath.Join(*out, "initial-state.json"), facts); err != nil {
			return err
		}
		meta["replay_identity"] = facts.Source
	}
	return writeJSON(filepath.Join(*out, "manifest.json"), meta)
}

func writeJSON(path string, value any) error {
	f, err := os.Create(path)
	if err != nil {
		return err
	}
	defer f.Close()
	enc := json.NewEncoder(f)
	enc.SetIndent("", "  ")
	return enc.Encode(value)
}
