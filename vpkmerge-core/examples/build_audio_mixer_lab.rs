//! Build a standalone in-game Audio Mixer Lab addon.
//!
//! Usage:
//! cargo run --example build_audio_mixer_lab -- <base pak01_dir.vpk> <qol_lock_dir.vpk> <out_dir.vpk>

use anyhow::{bail, Context, Result};
use std::path::PathBuf;

const ESCAPE_MENU_LAYOUT: &str = "panorama/layout/hud_escape_menu.vxml_c";
const SETTINGS_STYLE: &str = "panorama/styles/ql_settings.vcss_c";
const AUDIO_SCRIPT: &str = "panorama/scripts/ql_settings.vjs_c";
const SOUNDMIXERS_ENTRY: &str = "scripts/soundmixers.txt";
const STUB_SCRIPT: &str = "\n// [AudioMixerLab] unused QOL donor metadata stub.\n";
const STUB_SCRIPT_ENTRIES: &[&str] = &[
    "panorama/scripts/ql_shared_presets.vjs_c",
    "panorama/scripts/ql_custom_announcer_slot1_pack_meta.vjs_c",
    "panorama/scripts/ql_custom_announcer_slot2_pack_meta.vjs_c",
    "panorama/scripts/ql_custom_announcer_slot3_pack_meta.vjs_c",
    "panorama/scripts/ql_custom_announcer_slot4_pack_meta.vjs_c",
    "panorama/scripts/ql_custom_announcer_slot5_pack_meta.vjs_c",
];

const AUDIO_MIXER_LAB_JS: &str = r##"

// [AudioMixerLab] standalone in-game audio mixer and sound logging controls.
var AUDIO_MIXER_LAB_LOADED = (typeof AUDIO_MIXER_LAB_LOADED !== "undefined") ? AUDIO_MIXER_LAB_LOADED : false;
(function () {
    if (AUDIO_MIXER_LAB_LOADED) return;
    AUDIO_MIXER_LAB_LOADED = true;

    var PANEL_ID = "AudioMixerLabRoot";
    var USER_LAYER = "aml-user";

    var CORE_CHANNELS = [
        { label: "Master", command: "volume", value: 1.0 },
        { label: "Game / SFX", command: "snd_gamevolume", value: 1.0 },
        { label: "Music", command: "snd_musicvolume", value: 1.0 }
    ];

    var MIX_GROUPS = [
        { label: "Music", group: "Music", value: 0.5 },
        { label: "Music - Game Over", group: "Music-GameOver", value: 0.5 },
        { label: "Music - Objective", group: "Music-Objective", value: 0.5 },
        { label: "Announcer", group: "Announcer", value: 0.6 },
        { label: "UI", group: "UI", value: 0.5 },
        { label: "Weapons", group: "Weapons", value: 0.25 },
        { label: "Weapons - Opponent", group: "Weapons-Opp", value: 0.25 },
        { label: "Abilities - Player", group: "Ability-Pla", value: 0.6 },
        { label: "Abilities - Opponent", group: "Ability-Opp", value: 0.4 },
        { label: "Ultimates - Player", group: "Ult-Pla", value: 0.5 },
        { label: "Ultimates - Opponent", group: "Ult-Opp", value: 0.65 },
        { label: "Hero VO - Player", group: "VO-Hero-Pla", value: 1.0 },
        { label: "Hero VO - Opponent", group: "VO-Hero-Opp", value: 0.85 },
        { label: "Hero VO - Pings", group: "VO-Hero-Ping", value: 1.0 },
        { label: "NPC VO", group: "VO-NPC", value: 0.4 },
        { label: "Movement - Player", group: "Movement-Pla", value: 0.2 },
        { label: "Movement - Opponent", group: "Movement-Opp", value: 0.7 },
        { label: "Damage - Attacker", group: "Dmg-Attacker", value: 0.45 },
        { label: "Damage - Victim", group: "Dmg-Victim", value: 0.5 },
        { label: "Hit Feedback", group: "Hit-Attacker", value: 0.5 },
        { label: "Ambience Bed", group: "Ambience-Bed", value: 0.6 },
        { label: "Ambience Details", group: "Ambience-Details", value: 0.6 },
        { label: "Map Objective", group: "MapObjective", value: 0.75 }
    ];

    var PRESET_LAYERS = [
        { label: "Clear", layer: "", amount: 0.0 },
        { label: "Music Down", layer: "aml-music-down", amount: 1.0 },
        { label: "Voice Focus", layer: "aml-voice-focus", amount: 1.0 },
        { label: "Combat Focus", layer: "aml-combat-focus", amount: 1.0 },
        { label: "Ambience Off", layer: "aml-ambience-off", amount: 1.0 }
    ];

    function style(panel, values) {
        if (!panel || !panel.style) return panel;
        for (var key in values) {
            if (values.hasOwnProperty(key)) {
                try { panel.style[key] = values[key]; } catch (e0) {}
            }
        }
        return panel;
    }

    function setText(panel, text) {
        if (!panel) return;
        try { panel.text = text; } catch (e0) {}
    }

    function createLabel(parent, text, values) {
        var label = $.CreatePanel("Label", parent, "");
        label.text = text;
        style(label, values || {});
        return label;
    }

    function createButton(parent, text, values, onactivate) {
        var button = $.CreatePanel("Button", parent, "");
        style(button, values || {});
        createLabel(button, text, {
            horizontalAlign: "center",
            verticalAlign: "center",
            color: "#f1f5f9",
            fontSize: "15px",
            fontWeight: "bold"
        });
        if (onactivate) button.SetPanelEvent("onactivate", onactivate);
        return button;
    }

    function clamp01(value) {
        var number = Number(value);
        if (!isFinite(number)) number = 1.0;
        if (number < 0) number = 0;
        if (number > 1) number = 1;
        return number;
    }

    function formatValue(value) {
        var number = clamp01(value);
        var text = number.toFixed(2);
        return text.replace(/\.?0+$/, "");
    }

    function sanitizeConsoleArg(value) {
        var text = String(value === undefined || value === null ? "" : value);
        text = text.replace(/["\\;\n\r]/g, "");
        if (text.length > 96) text = text.substr(0, 96);
        return "\"" + text + "\"";
    }

    function sendCommand(command) {
        command = String(command || "").trim();
        if (!command) return false;
        var ok = false;
        try {
            $.DispatchEvent("CitadelConCommand", command);
            ok = true;
        } catch (e0) {}
        try { $.Msg("[AudioMixerLab] " + command); } catch (e1) {}
        appendCommandLog((ok ? "> " : "! ") + command);
        return ok;
    }

    function resetPresetLayers() {
        for (var i = 0; i < PRESET_LAYERS.length; i++) {
            if (PRESET_LAYERS[i].layer) {
                sendCommand("snd_soundmixer_setmixlayer_amount " + PRESET_LAYERS[i].layer + " 0");
            }
        }
    }

    var commandLogPanel = null;

    function appendCommandLog(line) {
        if (!commandLogPanel || !commandLogPanel.IsValid || !commandLogPanel.IsValid()) return;
        var children = commandLogPanel.Children();
        while (children && children.length > 11) {
            try { children[0].DeleteAsync(0); } catch (e0) { break; }
            children = commandLogPanel.Children();
        }
        createLabel(commandLogPanel, line, {
            color: "#cbd5e1",
            fontSize: "12px",
            width: "100%",
            whiteSpace: "nowrap",
            textOverflow: "shrink"
        });
    }

    function applyCoreChannel(def, value) {
        var next = clamp01(value);
        sendCommand(def.command + " " + formatValue(next));
    }

    function applyMixGroup(def, value) {
        var next = clamp01(value);
        sendCommand("snd_setmixlayer " + def.group + " " + USER_LAYER + " vol " + formatValue(next) + " 1");
        sendCommand("snd_soundmixer_setmixlayer_amount " + USER_LAYER + " 1");
    }

    function syncSlider(slider, input, value) {
        var next = clamp01(value);
        if (slider && Math.round(Number(slider.value)) !== Math.round(next * 100)) {
            slider.value = Math.round(next * 100);
        }
        if (input) input.text = formatValue(next);
    }

    function createVolumeRow(parent, def, applyFn, options) {
        var applyLabel = options && options.applyLabel ? options.applyLabel : "Apply";
        var row = $.CreatePanel("Panel", parent, "");
        style(row, {
            flowChildren: "right",
            width: "100%",
            minHeight: "36px",
            marginTop: "4px"
        });

        createLabel(row, def.label, {
            width: "132px",
            verticalAlign: "center",
            color: "#e2e8f0",
            fontSize: "14px",
            textOverflow: "shrink"
        });

        var sliderWrap = $.CreatePanel("Panel", row, "");
        style(sliderWrap, {
            width: "136px",
            height: "30px",
            verticalAlign: "center"
        });

        var slider = $.CreatePanel("Slider", sliderWrap, "", { direction: "horizontal" });
        style(slider, {
            width: "100%",
            height: "28px",
            verticalAlign: "center"
        });
        slider.min = 0;
        slider.max = 100;
        slider.value = Math.round(clamp01(def.value) * 100);

        var input = $.CreatePanel("TextEntry", row, "");
        input.text = formatValue(def.value);
        input.maxchars = 4;
        style(input, {
            width: "48px",
            height: "28px",
            verticalAlign: "center",
            backgroundColor: "#0f172aee",
            border: "1px solid #334155",
            color: "#e2e8f0",
            fontSize: "13px",
            marginLeft: "8px",
            paddingLeft: "5px",
            paddingRight: "5px"
        });

        slider.SetPanelEvent("onvaluechanged", function () {
            var next = clamp01(Number(slider.value) / 100);
            if (input) input.text = formatValue(next);
        });

        input.SetPanelEvent("oninputsubmit", function () {
            var raw = String(input.text || "").replace(",", ".");
            var parsed = parseFloat(raw);
            if (!isFinite(parsed)) parsed = def.value;
            if (parsed > 1) parsed = parsed / 100;
            var next = clamp01(parsed);
            syncSlider(slider, input, next);
        });

        createButton(row, applyLabel, {
            width: "58px",
            height: "26px",
            verticalAlign: "center",
            marginLeft: "6px",
            backgroundColor: "#14532d",
            border: "1px solid #22c55e"
        }, function () {
            var next = clamp01(Number(slider.value) / 100);
            applyFn(def, next);
        });

        createButton(row, "Reset", {
            width: "52px",
            height: "26px",
            verticalAlign: "center",
            marginLeft: "4px",
            backgroundColor: "#334155",
            border: "1px solid #475569"
        }, function () {
            syncSlider(slider, input, def.value);
            applyFn(def, def.value);
        });
    }

    function createSection(parent, title) {
        var section = $.CreatePanel("Panel", parent, "");
        style(section, {
            flowChildren: "down",
            width: "100%",
            marginTop: "10px",
            paddingTop: "8px",
            borderTop: "1px solid #334155"
        });
        createLabel(section, title, {
            color: "#f8fafc",
            fontSize: "16px",
            fontWeight: "bold",
            marginBottom: "4px"
        });
        return section;
    }

    function buildCoreSection(parent) {
        var section = createSection(parent, "Core Channels");
        for (var i = 0; i < CORE_CHANNELS.length; i++) {
            createVolumeRow(section, CORE_CHANNELS[i], applyCoreChannel, { applyLabel: "Set" });
        }
    }

    function buildMixGroupSection(parent) {
        var section = createSection(parent, "Audio Mixer Layer");
        createLabel(section, "Values apply to the aml-user mix layer from scripts/soundmixers.txt.", {
            color: "#94a3b8",
            fontSize: "12px",
            marginBottom: "4px"
        });
        var controlRow = $.CreatePanel("Panel", section, "");
        style(controlRow, { flowChildren: "right", width: "100%", marginTop: "4px", marginBottom: "4px" });
        createButton(controlRow, "Enable Layer", {
            width: "112px",
            height: "30px",
            marginRight: "6px",
            backgroundColor: "#14532d",
            border: "1px solid #22c55e"
        }, function () {
            sendCommand("snd_soundmixer_setmixlayer_amount " + USER_LAYER + " 1");
        });
        createButton(controlRow, "Disable Layer", {
            width: "116px",
            height: "30px",
            marginRight: "6px",
            backgroundColor: "#7f1d1d",
            border: "1px solid #ef4444"
        }, function () {
            sendCommand("snd_soundmixer_setmixlayer_amount " + USER_LAYER + " 0");
        });
        createButton(controlRow, "Flush Mixers", {
            width: "108px",
            height: "30px",
            backgroundColor: "#334155",
            border: "1px solid #475569"
        }, function () {
            sendCommand("snd_soundmixer_flush");
        });
        for (var i = 0; i < MIX_GROUPS.length; i++) {
            createVolumeRow(section, MIX_GROUPS[i], applyMixGroup, { applyLabel: "Apply" });
        }
    }

    function buildPresetSection(parent) {
        var section = createSection(parent, "Mixer Presets");
        var row = $.CreatePanel("Panel", section, "");
        style(row, {
            flowChildren: "right-wrap",
            width: "100%",
            marginTop: "6px"
        });
        for (var i = 0; i < PRESET_LAYERS.length; i++) {
            (function (preset) {
                createButton(row, preset.label, {
                    width: "112px",
                    height: "30px",
                    marginRight: "6px",
                    marginBottom: "6px",
                    backgroundColor: preset.layer ? "#334155" : "#7f1d1d",
                    border: "1px solid #475569"
                }, function () {
                    resetPresetLayers();
                    if (preset.layer) {
                        sendCommand("snd_soundmixer_setmixlayer_amount " + preset.layer + " " + formatValue(preset.amount));
                    }
                });
            })(PRESET_LAYERS[i]);
        }
    }

    function buildLoggingSection(parent) {
        var section = createSection(parent, "Sound Logging");

        var filterRow = $.CreatePanel("Panel", section, "");
        style(filterRow, {
            flowChildren: "right",
            width: "100%",
            marginTop: "6px"
        });
        createLabel(filterRow, "Filter", {
            width: "80px",
            verticalAlign: "center",
            color: "#e2e8f0",
            fontSize: "14px"
        });
        var filterInput = $.CreatePanel("TextEntry", filterRow, "AudioMixerLabSoundFilter");
        filterInput.text = "";
        filterInput.maxchars = 96;
        style(filterInput, {
            width: "292px",
            height: "30px",
            backgroundColor: "#0f172aee",
            border: "1px solid #334155",
            color: "#e2e8f0",
            fontSize: "13px",
            paddingLeft: "6px",
            paddingRight: "6px"
        });

        function applyFilter() {
            var filter = sanitizeConsoleArg(filterInput.text || "");
            sendCommand("snd_sos_soundevent_filter " + filter);
            sendCommand("snd_filter " + filter);
        }

        filterInput.SetPanelEvent("oninputsubmit", applyFilter);

        var rowA = $.CreatePanel("Panel", section, "");
        style(rowA, { flowChildren: "right", width: "100%", marginTop: "8px" });
        createButton(rowA, "Start", {
            width: "88px",
            height: "30px",
            marginRight: "6px",
            backgroundColor: "#166534",
            border: "1px solid #22c55e"
        }, function () {
            sendCommand("snd_sos_show_soundevent_start 1");
            applyFilter();
        });
        createButton(rowA, "Stop", {
            width: "88px",
            height: "30px",
            marginRight: "6px",
            backgroundColor: "#7f1d1d",
            border: "1px solid #ef4444"
        }, function () {
            sendCommand("snd_sos_show_soundevent_start 0");
            sendCommand("cl_snd_new_visualize 0");
        });
        createButton(rowA, "Apply Filter", {
            width: "120px",
            height: "30px",
            backgroundColor: "#334155",
            border: "1px solid #475569"
        }, applyFilter);

        var rowB = $.CreatePanel("Panel", section, "");
        style(rowB, { flowChildren: "right", width: "100%", marginTop: "6px" });
        createButton(rowB, "3D Labels", {
            width: "96px",
            height: "30px",
            marginRight: "6px",
            backgroundColor: "#334155",
            border: "1px solid #475569"
        }, function () {
            sendCommand("cl_snd_new_visualize 1");
        });
        createButton(rowB, "No Labels", {
            width: "96px",
            height: "30px",
            marginRight: "6px",
            backgroundColor: "#334155",
            border: "1px solid #475569"
        }, function () {
            sendCommand("cl_snd_new_visualize 0");
        });
        createButton(rowB, "List Events", {
            width: "102px",
            height: "30px",
            backgroundColor: "#334155",
            border: "1px solid #475569"
        }, function () {
            sendCommand("snd_list_soundevents");
        });

        var rowC = $.CreatePanel("Panel", section, "");
        style(rowC, { flowChildren: "right", width: "100%", marginTop: "6px" });
        createButton(rowC, "List Mix Groups", {
            width: "132px",
            height: "30px",
            marginRight: "6px",
            backgroundColor: "#334155",
            border: "1px solid #475569"
        }, function () {
            sendCommand("snd_soundmixer_list_mix_groups");
        });
        createButton(rowC, "List Layers", {
            width: "104px",
            height: "30px",
            backgroundColor: "#334155",
            border: "1px solid #475569"
        }, function () {
            sendCommand("snd_soundmixer_list_mix_layers");
        });

        commandLogPanel = $.CreatePanel("Panel", section, "AudioMixerLabCommandLog");
        style(commandLogPanel, {
            flowChildren: "down",
            width: "100%",
            height: "112px",
            marginTop: "8px",
            backgroundColor: "#020617dd",
            border: "1px solid #1e293b",
            padding: "6px",
            overflow: "squish scroll"
        });
    }

    function deleteChildren(panel) {
        if (!panel || !panel.Children) return;
        var children = [];
        try { children = panel.Children() || []; } catch (e0) { children = []; }
        for (var i = 0; i < children.length; i++) {
            try { children[i].DeleteAsync(0); } catch (e1) {}
        }
    }

    function buildPanel(root) {
        if (!root || !root.IsValid || !root.IsValid()) return false;
        var win = root.FindChildTraverse("SettingsWindow");
        var list = root.FindChildTraverse("SettingsList");
        if (!win || !list || !win.IsValid || !win.IsValid() || !list.IsValid || !list.IsValid()) return false;

        var title = win.FindChildTraverse("SettingsTitle");
        if (title) title.text = "MIXER";
        var accent = win.FindChildTraverse("SettingsTitleAccent");
        if (accent) accent.text = "AUDIO";

        var existing = list.FindChildTraverse(PANEL_ID);
        if (existing && existing.IsValid && existing.IsValid()) return true;
        deleteChildren(list);

        var body = $.CreatePanel("Panel", list, PANEL_ID);
        style(body, {
            flowChildren: "down",
            width: "100%",
            height: "100%",
            padding: "12px",
            backgroundColor: "#020617f2",
            overflow: "squish scroll"
        });

        createLabel(body, "Audio Mixer Lab", {
            color: "#f8fafc",
            fontSize: "20px",
            fontWeight: "bold",
            marginBottom: "2px"
        });
        createLabel(body, "Standalone mixer layers are loaded from scripts/soundmixers.txt.", {
            color: "#94a3b8",
            fontSize: "12px",
            marginBottom: "4px"
        });

        buildCoreSection(body);
        buildPresetSection(body);
        buildMixGroupSection(body);
        buildLoggingSection(body);

        appendCommandLog("ready");
        return true;
    }

    function getRoot() {
        var root = null;
        try { root = $.GetContextPanel(); } catch (e0) { root = null; }
        return root;
    }

    $.BuildUI = function() {
        return buildPanel(getRoot());
    };

    $.ToggleSettingsWindow = function() {
        var root = getRoot();
        if (!root) return;
        var win = root.FindChildTraverse("SettingsWindow");
        if (!win) return;
        buildPanel(root);
        win.ToggleClass("Visible");
        try {
            if (win.BHasClass && win.BHasClass("Visible")) win.SetFocus();
        } catch (e0) {}
    };

    $.ForceCloseModSettings = function() {
        var root = getRoot();
        if (!root) return;
        var win = root.FindChildTraverse("SettingsWindow");
        if (win) win.RemoveClass("Visible");
        try { $.DispatchEvent("CitadelResumePlaying", root); } catch (e0) {}
    };

    function installWhenReady(triesLeft) {
        if (buildPanel(getRoot())) return;
        if (triesLeft <= 0) {
            try { $.Msg("[AudioMixerLab] failed to attach settings panel"); } catch (e1) {}
            return;
        }
        $.Schedule(0.25, function () { installWhenReady(triesLeft - 1); });
    }

    $.Schedule(0.1, function () { installWhenReady(20); });
})();
"##;

const AUDIO_MIXER_LAB_MIXLAYERS: &str = r#"
		"aml-user" = 
		{
			Mixers = 
			[
				{ mixgroup = "Music" vol = 0.498884 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Music-GameOver" vol = 0.5 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Music-Objective" vol = 0.5 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Announcer" vol = 0.6 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "UI" vol = 0.498884 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Weapons" vol = 0.25 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Weapons-Opp" vol = 0.25 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Ability-Pla" vol = 0.6 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Ability-Opp" vol = 0.4 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Ult-Pla" vol = 0.5 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Ult-Opp" vol = 0.65 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "VO-Hero-Pla" vol = 1.0 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "VO-Hero-Opp" vol = 0.85 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "VO-Hero-Ping" vol = 1.0 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "VO-NPC" vol = 0.4 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Movement-Pla" vol = 0.2 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Movement-Opp" vol = 0.7 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Dmg-Attacker" vol = 0.45 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Dmg-Victim" vol = 0.5 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Hit-Attacker" vol = 0.5 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Ambience-Bed" vol = 0.6 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Ambience-Details" vol = 0.6 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "MapObjective" vol = 0.75 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
			]
			Triggers = [  ]
		}
		"aml-music-down" = 
		{
			Mixers = 
			[
				{ mixgroup = "Music" vol = 0.15 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Music-GameOver" vol = 0.15 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Music-KS" vol = 0.15 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Music-Objective" vol = 0.15 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
			]
			Triggers = [  ]
		}
		"aml-voice-focus" = 
		{
			Mixers = 
			[
				{ mixgroup = "Music" vol = 0.25 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Music-GameOver" vol = 0.25 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Ambience-Bed" vol = 0.25 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Ambience-Details" vol = 0.25 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Weapons" vol = 0.45 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Ability-Pla" vol = 0.45 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Ability-Opp" vol = 0.45 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "VO-Hero-Pla" vol = 1.0 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "VO-Hero-Opp" vol = 1.0 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "VO-Hero-Ping" vol = 1.0 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "VO-NPC" vol = 0.85 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Announcer" vol = 0.9 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
			]
			Triggers = [  ]
		}
		"aml-combat-focus" = 
		{
			Mixers = 
			[
				{ mixgroup = "Music" vol = 0.2 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Music-GameOver" vol = 0.2 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Ambience-Bed" vol = 0.25 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Ambience-Details" vol = 0.25 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Weapons" vol = 0.85 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Weapons-Opp" vol = 0.85 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Ability-Pla" vol = 0.85 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Ability-Opp" vol = 0.75 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Ult-Pla" vol = 0.85 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Ult-Opp" vol = 0.9 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Dmg-Attacker" vol = 0.85 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Dmg-Victim" vol = 0.85 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Hit-Attacker" vol = 0.85 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
			]
			Triggers = [  ]
		}
		"aml-ambience-off" = 
		{
			Mixers = 
			[
				{ mixgroup = "Ambience-Bed" vol = 0.0 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
				{ mixgroup = "Ambience-Details" vol = 0.0 solo = 0.0 mute = 0.0 lvl = 0.0 dsp = 1.0 },
			]
			Triggers = [  ]
		}
"#;

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let end = offset
        .checked_add(4)
        .context("u32 offset overflow while reading resource")?;
    let raw = bytes
        .get(offset..end)
        .with_context(|| format!("resource truncated while reading u32 at {offset}"))?;
    Ok(u32::from_le_bytes(raw.try_into()?))
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) -> Result<()> {
    let end = offset
        .checked_add(4)
        .context("u32 offset overflow while writing resource")?;
    let raw = bytes
        .get_mut(offset..end)
        .with_context(|| format!("resource truncated while writing u32 at {offset}"))?;
    raw.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

#[derive(Clone, Copy)]
struct BlockRef {
    kind: [u8; 4],
    offset: usize,
    size: usize,
}

fn read_blocks(resource: &[u8]) -> Result<Vec<BlockRef>> {
    if resource.len() < 16 {
        bail!("resource is too small to contain a Source 2 block table");
    }

    let block_table_offset = 8usize
        .checked_add(read_u32(&resource, 8)? as usize)
        .context("block table offset overflow")?;
    let block_count = read_u32(&resource, 12)? as usize;
    let mut blocks = Vec::with_capacity(block_count);

    for block_index in 0..block_count {
        let entry_offset = block_table_offset
            .checked_add(block_index * 12)
            .context("block table entry offset overflow")?;
        let entry_end = entry_offset
            .checked_add(12)
            .context("block table entry end overflow")?;
        let block_entry = resource
            .get(entry_offset..entry_end)
            .with_context(|| format!("resource block table entry {block_index} is truncated"))?;
        let kind: [u8; 4] = block_entry[0..4].try_into()?;
        let relative_offset = read_u32(&resource, entry_offset + 4)? as usize;
        let size = read_u32(&resource, entry_offset + 8)? as usize;
        let absolute_offset = entry_offset
            .checked_add(4)
            .and_then(|offset| offset.checked_add(relative_offset))
            .context("block data offset overflow")?;
        let absolute_end = absolute_offset
            .checked_add(size)
            .context("block data end overflow")?;
        if absolute_end > resource.len() {
            bail!("resource block {block_index} extends past end of file");
        }
        blocks.push(BlockRef {
            kind,
            offset: absolute_offset,
            size,
        });
    }

    Ok(blocks)
}

fn align16(value: usize) -> usize {
    (value + 15) & !15
}

fn rebuild_with_replaced_block(
    resource: &[u8],
    target_kind: [u8; 4],
    new_payload: &[u8],
) -> Result<Vec<u8>> {
    let blocks = read_blocks(resource)?;
    let target_index = blocks
        .iter()
        .position(|block| block.kind == target_kind)
        .with_context(|| {
            format!(
                "resource has no {} block",
                String::from_utf8_lossy(&target_kind)
            )
        })?;
    let resource_version = u16::from_le_bytes(
        resource
            .get(6..8)
            .context("resource truncated before version")?
            .try_into()?,
    );

    let mut payloads = Vec::with_capacity(blocks.len());
    for (index, block) in blocks.iter().enumerate() {
        if index == target_index {
            payloads.push(new_payload);
            continue;
        }
        payloads.push(
            resource
                .get(block.offset..block.offset + block.size)
                .context("block payload slice out of range")?,
        );
    }

    let header_len = 16usize;
    let table_len = blocks
        .len()
        .checked_mul(12)
        .context("block table length overflow")?;
    let mut cursor = align16(
        header_len
            .checked_add(table_len)
            .context("resource header length overflow")?,
    );
    let mut offsets = Vec::with_capacity(payloads.len());
    for payload in &payloads {
        offsets.push(cursor);
        cursor = align16(
            cursor
                .checked_add(payload.len())
                .context("block payload offset overflow")?,
        );
    }

    let mut out = vec![0u8; cursor];
    write_u32(
        &mut out,
        0,
        u32::try_from(cursor).context("resource too large")?,
    )?;
    out[4..6].copy_from_slice(&12u16.to_le_bytes());
    out[6..8].copy_from_slice(&resource_version.to_le_bytes());
    write_u32(&mut out, 8, 8)?;
    write_u32(
        &mut out,
        12,
        u32::try_from(blocks.len()).context("too many blocks")?,
    )?;

    for (index, block) in blocks.iter().enumerate() {
        let entry = header_len + index * 12;
        out[entry..entry + 4].copy_from_slice(&block.kind);
        let offset_field = entry + 4;
        let relative = offsets[index]
            .checked_sub(offset_field)
            .context("block relative offset underflow")?;
        write_u32(
            &mut out,
            offset_field,
            u32::try_from(relative).context("block relative offset overflow")?,
        )?;
        write_u32(
            &mut out,
            offset_field + 4,
            u32::try_from(payloads[index].len()).context("block too large")?,
        )?;
    }

    for (offset, payload) in offsets.iter().zip(&payloads) {
        out[*offset..*offset + payload.len()].copy_from_slice(payload);
    }

    Ok(out)
}

fn vjs_with_script_data(donor: &[u8], script: &[u8]) -> Result<Vec<u8>> {
    rebuild_with_replaced_block(donor, *b"DATA", script)
}

fn replace_same_length(mut bytes: Vec<u8>, from: &[u8], to: &[u8]) -> Result<Vec<u8>> {
    if from.len() != to.len() {
        bail!("same-length replacement size mismatch");
    }
    let mut offset = 0usize;
    while let Some(pos) = bytes[offset..]
        .windows(from.len())
        .position(|window| window == from)
    {
        let start = offset + pos;
        bytes[start..start + to.len()].copy_from_slice(to);
        offset = start + to.len();
    }
    Ok(bytes)
}

fn soundmixers_with_audio_lab_layers(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut text = String::from_utf8(bytes.to_vec()).context("soundmixers.txt is not UTF-8")?;
    if text.contains("\"aml-user\"") {
        return Ok(bytes.to_vec());
    }

    let insert_at = text
        .rfind("\r\n\t}\r\n}")
        .or_else(|| text.rfind("\n\t}\n}"))
        .context("could not find soundmixers.txt MixLayers close")?;
    text.insert_str(insert_at, AUDIO_MIXER_LAB_MIXLAYERS);
    Ok(text.into_bytes())
}

fn main() -> Result<()> {
    let usage =
        "usage: cargo run --example build_audio_mixer_lab -- <base pak01_dir.vpk> <qol_lock_dir.vpk> <out_dir.vpk>";
    let mut args = std::env::args_os().skip(1);
    let base_vpk = PathBuf::from(args.next().context(usage)?);
    let qol_vpk = PathBuf::from(args.next().context(usage)?);
    let output_vpk = PathBuf::from(args.next().context(usage)?);
    if args.next().is_some() {
        bail!("too many arguments\n{usage}");
    }

    let escape_menu = vpkmerge_core::read_vpk_entry(&qol_vpk, ESCAPE_MENU_LAYOUT)
        .with_context(|| format!("reading {ESCAPE_MENU_LAYOUT} from {}", qol_vpk.display()))?;
    let escape_menu = replace_same_length(escape_menu, b"QOL LOCK", b"AUDIO MX")?;

    let style = vpkmerge_core::read_vpk_entry(&qol_vpk, SETTINGS_STYLE)
        .with_context(|| format!("reading {SETTINGS_STYLE} from {}", qol_vpk.display()))?;

    let audio_donor = vpkmerge_core::read_vpk_entry(&qol_vpk, AUDIO_SCRIPT)
        .with_context(|| format!("reading {AUDIO_SCRIPT} from {}", qol_vpk.display()))?;
    let audio_script = vjs_with_script_data(&audio_donor, AUDIO_MIXER_LAB_JS.as_bytes())?;

    let soundmixers = vpkmerge_core::read_vpk_entry(&base_vpk, SOUNDMIXERS_ENTRY)
        .with_context(|| format!("reading {SOUNDMIXERS_ENTRY} from {}", base_vpk.display()))?;
    let soundmixers = soundmixers_with_audio_lab_layers(&soundmixers)?;

    let mut files: Vec<(String, Vec<u8>)> = vec![
        (ESCAPE_MENU_LAYOUT.to_string(), escape_menu),
        (SETTINGS_STYLE.to_string(), style),
        (AUDIO_SCRIPT.to_string(), audio_script),
        (SOUNDMIXERS_ENTRY.to_string(), soundmixers),
    ];

    for entry in STUB_SCRIPT_ENTRIES {
        let donor = vpkmerge_core::read_vpk_entry(&qol_vpk, entry)
            .with_context(|| format!("reading {entry} from {}", qol_vpk.display()))?;
        let stub = vjs_with_script_data(&donor, STUB_SCRIPT.as_bytes())
            .with_context(|| format!("building stub script {entry}"))?;
        files.push(((*entry).to_string(), stub));
    }

    let pack_files: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(entry, bytes)| (entry.as_str(), bytes.as_slice()))
        .collect();
    vpkmerge_core::pack(&pack_files, &output_vpk)
        .with_context(|| format!("packing {}", output_vpk.display()))?;

    println!("packed {} entries", pack_files.len());
    println!("patched {ESCAPE_MENU_LAYOUT} from QOL donor");
    println!("generated {AUDIO_SCRIPT}: {} bytes", pack_files[2].1.len());
    println!("patched {SOUNDMIXERS_ENTRY} with Audio Mixer Lab layers");
    println!("wrote {}", output_vpk.display());
    Ok(())
}
