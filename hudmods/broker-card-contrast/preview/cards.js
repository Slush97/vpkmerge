// Tooltip trees mirroring citadel_tooltip_mod_details.vxml, its snippets and
// citadel_modified_attribute_label.vxml, filled the way the game fills them for the two
// corrupted items captured in-game. Classes the C++ side adds at runtime are inferred
// from the stylesheets and the screenshots.
(function () {
  "use strict";

  const val = (num, unit) => (unit ? `${num}<span class="PostfixValue">${unit}</span>` : `${num}`);

  function attrLabel({ id, cls = "", value, unit, name = "", modified = false }) {
    const html = val(value, unit);
    return ["CitadelModifiedAttributeLabel", { id, class: cls, "data-layout": "attr" },
      ["Panel", { class: "LabelsContainer" },
        ["Panel", { class: "StatImageArea" }, ["Panel", { id: "StatImage", class: "StatImage" }], ["Label", { class: "NewOverride", text: "NEW" }]],
        ["Panel", { class: "LabelsText" },
          ["Label", { id: "BaseLabel", html: modified ? "" : html }],
          ["Label", { id: "ModifiedLabel", html: modified ? html : "" }],
          ["Label", { id: "ModifiedAdditionLabel", text: "+" }],
          ["Label", { id: "StatScalingLabel", html: "" }],
          ["Label", { id: "AdditionLabel", text: "+" }],
          ["Label", { id: "ValueFromModsLabel", html: "" }],
          ["Panel", { id: "TierBonusStatImage" }]],
        ["Panel", { class: "brawl_modifier" }],
        ["Panel", { class: "GreenUpArrow" }]],
      ["Label", { id: "AdditionalText", html: name }],
      ["Panel", { class: "BonusDiff LeftRightFlow" },
        ["Label", { class: "PreBonusValue", html: "" }],
        ["Image", { class: "DiffArrow", src: "panorama/images/upgrades/arrow_delta_psd" }],
        ["Label", { class: "PostBonusValue", html: "" }]],
      ["Label", { class: "corrupted_rng", text: "Randomized!" }]];
  }

  // A corrupted stat row. kind: "up" | "down"; tier 1-3 picks the arrow art.
  const corrupted = (value, unit, name, kind = "up", tier = 1) =>
    attrLabel({ cls: kind === "down" ? `Corrupted isNegative Elevated CorruptedDown${tier}` : `Corrupted CorruptedUp${tier}`, value, unit, name });
  const plain = (value, unit, name) => attrLabel({ cls: "HasModifiedValue", value, unit, name, modified: true });

  function box(n, propClass, value, unit, type, imageClass = "") {
    return ["Panel", { id: "ImportantAttribute", class: `ImportantStatBox TopBottomFlow Corrupted ImportantProperty${n} ${propClass}` },
      ["CitadelStatScalingLabel", { id: "ScalingStatValue" }],
      ["Panel", { class: "ImportantStatContent" },
        ["Panel", { class: "ImportantStatImageValueContainer LeftRightFlow" },
          ["Panel", { class: `ImportantStatImage ${imageClass}` }],
          attrLabel({ id: "ImportantStatValue", cls: "Corrupted CorruptedUp1", value, unit })],
        ["Panel", { class: "ImportantStatLabelsContainer" },
          ["Label", { class: "ImportantStatType", text: type }],
          ["Label", { class: "ImportantStatLabel", text: "" }]]]];
  }

  function section(kind, { label, cooldown, boxes = [], rows = [], desc = "", cls = "" }) {
    const multi = boxes.length > 1 ? "HasMultipleImportantProperties" : "";
    const attrCls = kind === "Innate" ? "HideImportantAttribute HideDescription" : [multi, boxes.length ? "" : "HideImportantAttribute"].join(" ");
    return ["Panel", { id: `AbilityType_${kind}`, class: `AppliedAttributes TopBottomFlow ${cooldown ? "hasCooldown" : ""} ${cls}` },
      ["Panel", { class: "CooldownHeader LeftRightFlow" },
        ["Label", { class: "ActivePassiveLabel", text: label || kind }],
        ["Panel", { class: "HeaderAttributesContainer" },
          ["Panel", { class: "StacksContainer LeftRightFlow" }, ["Label", { class: "StacksDescLabel", text: "" }], ["Label", { id: "StacksLabel", text: "" }]],
          ["Panel", { class: "CooldownContainer LeftRightFlow" },
            ["Panel", { class: "CooldownImage" }],
            cooldown ? attrLabel({ id: "CooldownLabel", cls: cooldown.corrupted ? "Corrupted CorruptedUp1" : "", value: cooldown.value, unit: "s" }) : attrLabel({ id: "CooldownLabel", value: "" })],
          ["Panel", { class: "ChargeUpContainer LeftRightFlow" }, ["Panel", { class: "ChargeUpImage" }], attrLabel({ id: "ChargeUpLabel", value: "" })]]],
      ["Panel", { id: "AbilityTypeSection", class: "TopBottomFlow" },
        ["Panel", { id: `AttributesSection_${kind}`, class: `AppliedAttributesContainer TopBottomFlow ${attrCls}` },
          ["Panel", { class: "LeftRightFlow" }, ["Label", { id: "ExtraInfo", class: "ModInfoLabel", html: desc }]],
          ["Panel", { class: "StatsAppliedBackground" },
            ["Panel", { id: "ImportantAttributeContainer" }, ...boxes],
            ["Panel", { id: "StatsAppliedContainer" }, ...rows]]]]];
  }

  function tooltip({ theme, name, rootClass = "", sections, component }) {
    const themeClass = { weapon: "WeaponMod", vitality: "ArmorMod", spirit: "TechMod" }[theme];
    return ["CitadelTooltipModDetails", { class: `IsCorrupted ${themeClass} TooltipVisible IsPurchased ${component ? "hasComponents" : ""} ${rootClass}` },
      ["Panel", { id: "PrimaryTooltipContainer" },
        ["Panel", { id: "TooltipMainColumn" },
          ["Panel", { id: "ModTooltipContainer" },
            ["Panel", { class: "HeaderContainer" },
              ["Panel", { id: "HeaderBGOverlay" }],
              ["Panel", { id: "EnhancedIndicator" }, ["Label", { text: "Enhanced" }], ["Panel", { class: "enhancedIcon" }]],
              ["Panel", { id: "CorruptedIndicator" }, ["Panel", { id: "CorruptedLabel", class: "SplitLabel" }, ["Label", { text: "Corrupted" }]]],
              ["Panel", { id: "HeaderContents" },
                ["Panel", { id: "ModImage", class: "mod_icon" }],
                ["Panel", { class: "TopBottomFlow ModNameContainer" },
                  ["Panel", { class: "LeftRightFlow" }, ["Label", { id: "ModName", class: "ModName", text: name }], ["Label", { id: "ModLevel", text: "" }]],
                  ["Panel", { class: "PurchasedStateContainer" },
                    ["Panel", { class: "ModCostContainer LeftRightFlow" }, ["Panel", { class: "goldIcon" }], ["Label", { class: "ModCost", text: "" }]],
                    ["Panel", { class: "ModPurchasedContainer" },
                      ["Panel", { class: "LeftRightFlow purchased_label" }, ["Label", { class: "ModPurchasedLabel", text: "Owned" }], ["Panel", { class: "ModPurchasedCheck" }]],
                      ["Panel", { class: "LeftRightFlow snowball_level" }, ["Label", { class: "ModPurchasedLabel", text: "" }]],
                      ["Panel", { id: "ModImbuedContainer", class: "TopBottomFlow" }]]]]],
              ["Panel", { id: "TierBonusContainer", class: "TopBottomFlow" }],
              ["Panel", { id: "TierContainer" }, ["Panel", { class: "tier_bg" }], ["Panel", { id: "mod_tier_label" }]]],
            ["Panel", { class: "AttributesContainer" },
              ["Panel", { id: "SectionsContainer", class: "TopBottomFlow" }, ...sections],
              ["Panel", { id: "BrawlMsg" }]],
            ["CitadelAltInfoPanel", { id: "AltInfoContainer" }],
            ["Panel", { id: "OwnedByContainer" }],
            ["Panel", { id: "PopularityContainer" }],
            ["Panel", { class: "componentsSection" },
              ["Panel", { id: "ComponentsContainer", class: "componentGroup" },
                ["Label", { class: "componentsLabel", text: "Upgrades From:" }],
                ["Panel", { id: "ComponentsList", class: "componentsList" },
                  component ? ["Panel", { class: "componentMod componentOwned" },
                    ["Panel", { class: "componentModIconContainer" }, ["Image", { id: "componentModIcon", src: component.icon }], ["Panel", { class: "componentOwnedIcon" }]],
                    ["Label", { class: "componentModName", text: component.name }]] : null]],
              ["Panel", { id: "ComponentOfContainer", class: "componentGroup" }],
              ["Panel", { id: "ComponentUsedContainer", class: "componentGroup" }]]],
          ["Panel", { class: "BuildInfoContainer" }]],
        ["Panel", { class: "AffectedAbilitiesContainer TopBottomFlow" }]]];
  }

  window.CARDS = {
    frenzy: {
      label: "Frenzy", theme: "weapon", ref: "ref/frenzy.png", refOverride: 18,
      tree: () => tooltip({
        theme: "weapon", name: "Frenzy",
        sections: [
          section("Innate", { rows: [
            corrupted("-1", "", "Stamina", "down"),
            corrupted("-24", "%", "Stamina Regen", "down"),
            corrupted("+400", "", "Bonus Health"),
            corrupted("+25", "%", "Fire Rate"),
            corrupted("+25", "%", "Bullet Lifesteal"),
          ] }),
          section("Passive", {
            cooldown: { value: "16" },
            desc: 'While you are <span class="highlight">below 50% health</span>, you gain stat bonuses for a duration and existing debuffs on you are reduced.',
            boxes: [
              box(1, "prop_move_speed", "6", " m/s", "Move Speed"),
              box(2, "prop_fire_rate", "65", "%", "Fire Rate"),
              box(3, "prop_duration", "+60", "%", "Debuff Resist"),
            ],
            rows: [plain("8.00", "s", "Duration")],
          }),
        ],
      }),
    },
    focuslens: {
      label: "Focus Lens", theme: "spirit", ref: "ref/focus_lens.png", refOverride: 14,
      tree: () => tooltip({
        theme: "spirit", name: "Focus Lens",
        component: { name: "Spirit Sap", icon: "panorama/images/items/spirit/spirit_sap_psd" },
        sections: [
          section("Innate", { rows: [
            corrupted("-550", "", "Max Health", "down"),
            corrupted("+15", "%", "Fire Rate"),
          ] }),
          section("Active", {
            cooldown: { value: "35", corrupted: true },
            desc: 'Target an enemy to <span class="highlight">Silence</span> them. A portion of <span class="highlight">all damage dealt</span> during the silence gets applied to the target when the silence wears off.',
            boxes: [
              box(1, "prop_silence", "6.39", "s", "Silenced"),
              box(2, "prop_tech_damage", "55", "%", "Damage On Expire"),
            ],
            rows: [
              corrupted("-15", "%", "Spirit Resist"),
              plain("15.3", "s", "Resist Reduction Duration"),
              corrupted("35", "m", "Cast Range"),
              corrupted("-50", "", "Spirit Power"),
            ],
          }),
        ],
      }),
    },
  };
})();
