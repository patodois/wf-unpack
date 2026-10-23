//! Exports a [`LevelDescription`] to glTF: models are stored once and
//! instanced by nodes, materials are baked as for single models.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::rc::Rc;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::cache::Package;
use crate::gltf::{LevelWriter, ModelHandle};
use crate::gltf::{Material as GltfMaterial, Slot};
use crate::level::{InstanceRole, LevelDescription, Transform};
use crate::lightmap::LightmapPage;
use crate::material::{folder, resolve_path};
use crate::model::{Model, ModelHeader};
use crate::pbr::{Image, MaterialBaker};
use crate::physx::{read_shapes, Collider, Shape};
use crate::probe::{radiance_hdr, Probe};
use crate::scene;
use crate::texture::TexturePackages;

#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// Level of detail of every model (0 is the finest).
    pub lod: usize,
    /// Also place instances with `StartHidden=1`.
    pub include_hidden: bool,
    /// Bake textures; without it materials are flat colours.
    pub textures: bool,
    /// Shrink baked textures to at most this many pixels on a side.
    pub max_texture: Option<u32>,
    /// Write the lightmap pages the placed objects use as PNGs and describe them in the extras.
    pub lightmaps: bool,
    /// Decode the zones' reflection probes to equirectangular `.hdr` files.
    pub probes: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions {
            lod: 0,
            include_hidden: false,
            textures: true,
            max_texture: None,
            lightmaps: true,
            probes: true,
        }
    }
}

#[derive(Debug, Default)]
pub struct ExportReport {
    pub models_written: usize,
    pub meshes: usize,
    pub nodes: usize,
    pub lights: usize,
    /// Triangles stored once, and triangles the placed instances add up to.
    pub triangles_stored: u64,
    pub triangles_placed: u64,
    pub materials: usize,
    pub material_failures: Vec<(String, String)>,
    /// Models that could not be loaded, and the instances lost with them.
    pub model_failures: Vec<(String, String)>,
    pub instances_lost: usize,
    /// Models without triangles (collision or trigger shapes only), and their instances.
    pub no_geometry: Vec<String>,
    pub instances_without_geometry: usize,
    pub hidden_skipped: usize,
    pub buffer_bytes: usize,
    /// Lightmap pages written (a `_clr` and its `_dir` count as one), and pages that failed.
    pub lightmap_pages: usize,
    pub lightmap_failures: Vec<(String, String)>,
    pub probes_written: usize,
    pub probe_failures: Vec<(String, String)>,
}

/// Halves an image until it fits `max` (a box filter over 2x2 blocks).
fn shrink(img: &Image, max: u32) -> Image {
    let mut cur = img.clone();
    while cur.width.max(cur.height) > max && cur.width > 1 && cur.height > 1 {
        let (w, h) = (cur.width / 2, cur.height / 2);
        let mut out = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0u32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let i = (((2 * y + dy) * cur.width + 2 * x + dx) * 4) as usize;
                    for (a, &v) in acc.iter_mut().zip(&cur.rgba[i..i + 4]) {
                        *a += u32::from(v);
                    }
                }
                let o = ((y * w + x) * 4) as usize;
                for (dst, a) in out.rgba[o..o + 4].iter_mut().zip(acc) {
                    *dst = ((a + 2) / 4) as u8;
                }
            }
        }
        cur = out;
    }
    cur
}

struct Materials<'a> {
    baker: Option<MaterialBaker>,
    out_dir: &'a Path,
    opts: &'a ExportOptions,
    cache: HashMap<String, usize>,
}

impl Materials<'_> {
    /// The glTF material for a resolved material path.
    fn get(&mut self, w: &mut LevelWriter, path: &str, report: &mut ExportReport) -> usize {
        if let Some(&i) = self.cache.get(path) {
            return i;
        }
        let short = path.rsplit('/').next().unwrap_or(path).to_string();
        let mut mat = GltfMaterial::named(short.clone());
        if let Some(baker) = &mut self.baker {
            match baker.plan(path, path).and_then(|plan| {
                let baked = baker.bake(&plan)?;
                Ok((plan, baked))
            }) {
                Ok((_, baked)) => {
                    let n = self.cache.len();
                    for (slot, suffix, img) in [
                        (Slot::BaseColor, "base", &baked.base_color),
                        (
                            Slot::MetallicRoughness,
                            "metal_rough",
                            &baked.metallic_roughness,
                        ),
                        (Slot::Normal, "normal", &baked.normal),
                        (Slot::Emissive, "emissive", &baked.emissive),
                    ] {
                        let Some(img) = img else { continue };
                        let file = format!("m{n:03}_{short}_{suffix}.png");
                        let png = match self.opts.max_texture {
                            Some(max) if img.width.max(img.height) > max => {
                                shrink(img, max).to_png()
                            }
                            _ => img.to_png(),
                        };
                        match png.and_then(|p| Ok(fs::write(self.out_dir.join(&file), p)?)) {
                            Ok(()) => mat.images.push((slot, file)),
                            Err(e) => report
                                .material_failures
                                .push((path.to_string(), format!("{e:#}"))),
                        }
                    }
                    mat.base_color_factor = baked.base_color_factor;
                    mat.metallic_factor = baked.metallic_factor;
                    mat.roughness_factor = baked.roughness_factor;
                    mat.emissive_factor = baked.emissive_factor;
                    mat.emissive_strength = baked.emissive_strength;
                }
                Err(e) => report
                    .material_failures
                    .push((path.to_string(), format!("{e:#}"))),
            }
        }
        let i = w.add_material(&mat);
        self.cache.insert(path.to_string(), i);
        report.materials += 1;
        i
    }
}

/// glTF wants column-major matrices.
fn column_major(m: [[f32; 4]; 4]) -> Vec<f32> {
    (0..16).map(|k| m[k % 4][k / 4]).collect()
}

fn node_for(t: &Transform, name: &str, extras: Value) -> Value {
    let rh = t.to_right_handed();
    json!({"name": name, "matrix": column_major(rh.matrix()), "extras": extras})
}

fn short_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

struct Stored {
    handle: ModelHandle,
    /// The model's own material paths per slot, resolved.
    materials: Vec<String>,
    triangles: u64,
    slots: usize,
}

/// A lightmap page written next to the glTF.
#[derive(Debug, Clone)]
struct PageFiles {
    index: usize,
    clr_path: String,
    dir_path: Option<String>,
    clr_file: String,
    dir_file: Option<String>,
    clr_size: [usize; 2],
    dir_size: Option<[usize; 2]>,
}

fn png_rgba(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, width as u32, height as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header()?;
    writer.write_image_data(rgba)?;
    writer.finish()?;
    Ok(out)
}

/// Writes every lightmap page the placed instances use (`lm<N>_clr.png`,
/// `lm<N>_dir.png`, exactly the game's decoded texels) and returns them by name.
fn export_lightmap_pages(
    game: &Path,
    d: &LevelDescription,
    out_dir: &Path,
    include_hidden: bool,
    placed: &[bool],
    report: &mut ExportReport,
) -> HashMap<(String, Option<String>), PageFiles> {
    let mut pages: HashMap<(String, Option<String>), PageFiles> = HashMap::new();
    let Ok(mut pk) = TexturePackages::open_family(game, "LightMap") else {
        report
            .lightmap_failures
            .push(("H.LightMap".into(), "package not found".into()));
        return pages;
    };
    for (k, inst) in d.instances.iter().enumerate() {
        if !placed[k] || (inst.hidden && !include_hidden) {
            continue;
        }
        let Some(lm) = &inst.lightmap else { continue };
        let (Some(clr), dir) = lm.pages() else {
            continue;
        };
        let key = (clr.to_string(), dir.map(str::to_string));
        if pages.contains_key(&key) {
            continue;
        }
        let index = pages.len();
        let result = LightmapPage::load(&mut pk, clr, dir).and_then(|page| {
            let clr_file = format!("lm{index}_clr.png");
            fs::write(
                out_dir.join(&clr_file),
                png_rgba(page.clr.width, page.clr.height, &page.clr.rgba)?,
            )?;
            let (dir_file, dir_size) = match &page.dir {
                Some(a) => {
                    let f = format!("lm{index}_dir.png");
                    fs::write(out_dir.join(&f), png_rgba(a.width, a.height, &a.rgba)?)?;
                    (Some(f), Some([a.width, a.height]))
                }
                None => (None, None),
            };
            Ok(PageFiles {
                index,
                clr_path: clr.to_string(),
                dir_path: dir.map(str::to_string),
                clr_file,
                dir_file,
                clr_size: [page.clr.width, page.clr.height],
                dir_size,
            })
        });
        match result {
            Ok(files) => {
                report.lightmap_pages += 1;
                pages.insert(key, files);
            }
            Err(e) => report
                .lightmap_failures
                .push((clr.to_string(), format!("{e:#}"))),
        }
    }
    pages
}

/// Writes the indexed light map textures (`lm_idx<N>.png`, RGBA: one light per channel) the placed instances name.
fn export_indexed_pages(
    game: &Path,
    d: &LevelDescription,
    out_dir: &Path,
    placed: &[bool],
    include_hidden: bool,
    report: &mut ExportReport,
) -> HashMap<String, (String, [usize; 2])> {
    let mut out: HashMap<String, (String, [usize; 2])> = HashMap::new();
    let Ok(mut pk) = TexturePackages::open_family(game, "LightMap") else {
        return out;
    };
    for (k, inst) in d.instances.iter().enumerate() {
        if !placed[k] || (inst.hidden && !include_hidden) {
            continue;
        }
        let Some(lm) = &inst.lightmap else { continue };
        for ix in &lm.indexed {
            if out.contains_key(&ix.texture) {
                continue;
            }
            let file = format!("lm_idx{}.png", out.len());
            let result = pk.load(&ix.texture).and_then(|t| {
                let a = crate::lightmap::Atlas::from_texture(&t)?;
                fs::write(out_dir.join(&file), png_rgba(a.width, a.height, &a.rgba)?)?;
                Ok([a.width, a.height])
            });
            match result {
                Ok(size) => {
                    out.insert(ix.texture.clone(), (file, size));
                }
                Err(e) => report
                    .lightmap_failures
                    .push((ix.texture.clone(), format!("{e:#}"))),
            }
        }
    }
    out
}

/// Decodes the zones' reflection probes to `probe<N>_equirect.hdr`; returns the extras entries.
fn export_probes(
    game: &Path,
    d: &LevelDescription,
    out_dir: &Path,
    report: &mut ExportReport,
) -> Vec<Value> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for e in &d.environments {
        let Some(path) = &e.reflection_map else {
            continue;
        };
        if seen.contains(path) {
            continue;
        }
        seen.push(path.clone());
        let result = Probe::load(game, path).and_then(|p| {
            let (w, h) = (p.size * 4, p.size * 2);
            let file = format!("probe{}_equirect.hdr", seen.len() - 1);
            fs::write(out_dir.join(&file), radiance_hdr(w, h, &p.to_equirect(w, h))?)?;
            Ok(json!({
                "path": path, "equirect": file, "faceSize": p.size, "mipLevels": p.mip_levels,
                "meanClamped100": p.mean_clamped(100.0), "maxFinite": p.max_finite(),
                "convention": "game space (left-handed, y up); centre column looks along +Z, +X is to the right of centre; \
                               top row is +Y. Flip x for glTF's right-handed space.",
            }))
        });
        match result {
            Ok(j) => {
                report.probes_written += 1;
                out.push(j);
            }
            Err(e) => report.probe_failures.push((path.clone(), format!("{e:#}"))),
        }
    }
    out
}

/// What the glTF's `extras` say about the lightmap decode.
fn lightmap_format_doc() -> Value {
    json!({
        "summary": "Baked lighting, exactly as the game's pixel shaders decode it (see src/lightmap.rs).",
        "uv": "a = TEXCOORD_1 * lightmap.coords.zw + lightmap.coords.xy; no flip of v; textures clamp; the pages are the game's decoded texels",
        "clr": "RGBA: rgb chroma, a luma (stored squared)",
        "dir": "RG, two pages side by side (left page at u/2, right page at u/2 + 0.5)",
        "formula": [
            "clr = bspline(clr, a); dL = bspline(dir, (0.5 a.x, a.y)).rg; dR = bspline(dir, (0.5 a.x + 0.5, a.y)).rg",
            "chroma = clr.rgb * normalize.rgb; luma = clr.a^2 * normalize.w",
            "ambient = luma * chroma   (applied)",
            "d = (2 dL.x - 1, 2 dL.y - 1, 2 dR.x - 1); gain = dR.y^2 * S   (S unidentified: not applied)",
            "light = (luma + max(dot(n, gain * M d), 0)) * chroma   (linear radiance, scaled outside by PostProcessInfo.lightMapBoost and lightMapTint)",
            "diffuse = light * baseColor * (1 - metallic)",
        ],
        "mirroring": "The game's space is left-handed; the glTF nodes mirror x. d is in the game's (level) space: flip its x for glTF.",
        "notIdentified": [
            "S, the directional gain scale (not LightMapNormalize1.w); M, taken to be ProcRotation (identity in fixed levels)",
        ],
        "indexedLightMaps": "RGBA page, one light per channel of each IndexedLightMaps entry (lights in order): a per-light mask. The fill shaders evaluate up to 8 lights per object as mask * colour * max(1 - (w d^2)^2, 0)^2 * (1 + k (1 / (1 + d^2) - 1)) * N.L, with w = 1 / radius^2 inferred; how Brightness, Lumens, Color and PhysicalFalloff become colour and k is CPU code and not in the data.",
        "notDecoded": ["light probes (.lpd), sky spherical harmonics"],
    })
}

/// Writes `<name>.gltf` (+ `.bin` and PNGs) for the level.
pub fn export_level(
    game: &Path,
    d: &LevelDescription,
    out_dir: &Path,
    name: &str,
    opts: &ExportOptions,
) -> Result<ExportReport> {
    fs::create_dir_all(out_dir)?;
    let mut report = ExportReport::default();
    let mut w = LevelWriter::new();
    let mut head = Package::open_in(game, "H.Misc")?.context("no H.Misc")?;
    let mut body = Package::open_in(game, "B.Misc")?;
    let mut full = Package::open_in(game, "F.Misc")?;
    let mut materials = Materials {
        baker: if opts.textures {
            Some(MaterialBaker::open(game)?)
        } else {
            None
        },
        out_dir,
        opts,
        cache: HashMap::new(),
    };

    // Models, one at a time; only those some placed instance uses
    let used: Vec<bool> = {
        let mut u = vec![false; d.models.len()];
        for i in &d.instances {
            if opts.include_hidden || !i.hidden {
                u[i.model] = true;
            }
        }
        u
    };
    let mut stored: Vec<Option<Stored>> = Vec::with_capacity(d.models.len());
    for (m, path) in d.models.iter().enumerate() {
        if !used[m] {
            stored.push(None);
            continue;
        }
        let loaded =
            Model::load_from(&mut head, body.as_mut(), full.as_mut(), path).and_then(|model| {
                // A model with fewer LODs than asked for is drawn at its coarsest, not dropped
                let lod = model.usable_lod(opts.lod).unwrap_or(opts.lod);
                let prepared = scene::prepare(&model, lod, true)?;
                Ok((model, prepared))
            });
        match loaded {
            Ok((model, prepared)) if prepared.triangles() > 0 => {
                let handle = w.add_model(&prepared);
                let mats = model
                    .header
                    .materials()
                    .iter()
                    .map(|n| resolve_path(folder(path), n))
                    .collect();
                stored.push(Some(Stored {
                    handle,
                    materials: mats,
                    triangles: prepared.triangles() as u64,
                    slots: model.header.meshes.len(),
                }));
                report.models_written += 1;
                report.triangles_stored += prepared.triangles() as u64;
            }
            Ok(_) => {
                report.no_geometry.push(path.clone());
                stored.push(None);
            }
            Err(e) => {
                // The error chain starts with the path; the report has it already
                let text = format!("{e:#}");
                let text = text.strip_prefix(&format!("{path}: ")).unwrap_or(&text);
                let text = text.strip_prefix(&format!("{path}: ")).unwrap_or(text);
                report.model_failures.push((path.clone(), text.to_string()));
                stored.push(None);
            }
        }
    }

    // Lightmap pages of the instances that get a mesh
    let placed: Vec<bool> = d
        .instances
        .iter()
        .map(|i| stored[i.model].is_some())
        .collect();
    let lm_pages = if opts.lightmaps {
        export_lightmap_pages(game, d, out_dir, opts.include_hidden, &placed, &mut report)
    } else {
        HashMap::new()
    };

    let idx_pages = if opts.lightmaps {
        export_indexed_pages(game, d, out_dir, &placed, opts.include_hidden, &mut report)
    } else {
        HashMap::new()
    };

    // Meshes: one per model and material set
    let mut mesh_of: HashMap<(usize, Vec<String>), usize> = HashMap::new();
    for inst in &d.instances {
        if inst.hidden && !opts.include_hidden {
            report.hidden_skipped += 1;
            continue;
        }
        let Some(s) = &stored[inst.model] else {
            if report.no_geometry.contains(&d.models[inst.model]) {
                report.instances_without_geometry += 1;
            } else {
                report.instances_lost += 1;
            }
            continue;
        };
        let wanted: Vec<String> = (0..s.slots)
            .map(|slot| match inst.materials.get(slot) {
                Some(o) if !o.is_empty() => o.clone(),
                _ => s.materials.get(slot).cloned().unwrap_or_default(),
            })
            .collect();
        let key = (inst.model, wanted);
        let mesh = match mesh_of.get(&key) {
            Some(&m) => m,
            None => {
                let ids: Vec<Option<usize>> = key
                    .1
                    .iter()
                    .map(|p| (!p.is_empty()).then(|| materials.get(&mut w, p, &mut report)))
                    .collect();
                let m = w.add_mesh(short_name(&d.models[inst.model]), &s.handle, &ids);
                mesh_of.insert(key, m);
                report.meshes += 1;
                m
            }
        };
        let mut node = node_for(
            &inst.transform,
            &format!("{}{}", inst.scope, short_name(&d.models[inst.model])),
            json!({
                "type": inst.type_path,
                "scope": inst.scope,
                "object": inst.object,
                "hidden": inst.hidden,
                "zone": inst.role == InstanceRole::Zone,
                "model": d.models[inst.model],
                "lightmap": inst.lightmap.as_ref().map(|l| {
                    let (clr, dir) = l.pages();
                    let files = clr.and_then(|c| lm_pages.get(&(c.to_string(), dir.map(str::to_string))));
                    json!({
                        "textures": l.textures, "coords": l.coords,
                        "normalize": l.normalize, "normalize1": l.normalize1,
                        "page": files.map(|f| f.index),
                        "indexed": l.indexed.iter().map(|ix| json!({
                            "lights": ix.lights, "lightIndices": ix.light_indices,
                            "texture": ix.texture, "coords": ix.coords,
                            "file": idx_pages.get(&ix.texture).map(|p| p.0.clone()),
                            "size": idx_pages.get(&ix.texture).map(|p| p.1),
                            "bakedFalloffMask": ix.baked_falloff_mask})).collect::<Vec<_>>(),
                    })
                }),
            }),
        );
        node["mesh"] = json!(mesh);
        w.add_node(node);
        report.nodes += 1;
        report.triangles_placed += s.triangles;
    }

    // Lights, spawns and volumes as nodes with the level's values in extras
    for l in &d.lights {
        let t = Transform {
            translation: l.position,
            rotation: l.rotation,
            ..Transform::IDENTITY
        };
        let rgb = l
            .color
            .map_or([1.0; 3], |c| [c[0] / 255.0, c[1] / 255.0, c[2] / 255.0]);
        let spot = l
            .angle
            .filter(|_| !l.light_type.as_deref().is_some_and(|t| t.contains("OMNI")));
        let mut light =
            json!({"type": if spot.is_some() { "spot" } else { "point" }, "color": rgb});
        if let Some(a) = spot {
            light["spot"] =
                json!({"outerConeAngle": (a.to_radians() / 2.0).min(std::f32::consts::FRAC_PI_2)});
        }
        if let Some(r) = l.radius {
            light["range"] = json!(r);
        }
        let idx = w.add_light(light);
        let mut node = node_for(
            &t,
            &format!("{}light {}", l.scope, short_name(&l.type_path)),
            json!({
                "light": true,
                "type": l.type_path, "scope": l.scope, "object": l.object,
                "color": l.color, "temperature": l.temperature, "colorMode": l.color_mode,
                "brightness": l.brightness, "lumens": l.lumens, "radius": l.radius,
                "angle": l.angle, "length": l.length, "width": l.width, "height": l.height,
                "falloff": l.falloff, "falloffExponent": l.falloff_exponent,
                "lightType": l.light_type, "texture": l.texture, "ies": l.ies,
                "on": l.on, "directional": l.directional, "direction": l.direction,
                "physicalFalloff": l.attributes.iter().find(|(k, _)| k == "PhysicalFalloff").map(|(_, v)| v.clone()),
                "indexLightmap": l.attributes.iter().find(|(k, _)| k == "IndexLightmap").map(|(_, v)| v.clone()),
                "lightCache": l.attributes.iter().find(|(k, _)| k == "LightCache").map(|(_, v)| v.clone()),
            }),
        );
        node["extensions"] = json!({"KHR_lights_punctual": {"light": idx}});
        w.add_node(node);
        report.lights += 1;
    }
    for s in &d.spawns {
        let t = Transform {
            translation: s.position,
            rotation: s.rotation,
            ..Transform::IDENTITY
        };
        w.add_node(node_for(
            &t,
            &format!("{}spawn {}", s.scope, short_name(&s.type_path)),
            json!({"spawn": format!("{:?}", s.kind), "type": s.type_path, "scope": s.scope,
                   "object": s.object, "tag": s.tag}),
        ));
    }
    for p in &d.portals {
        w.add_node(node_for(
            &p.transform,
            &format!("{}portal {}", p.scope, short_name(&p.type_path)),
            json!({"portal": true, "type": p.type_path, "scope": p.scope, "object": p.object,
                   "width": p.width, "height": p.height}),
        ));
    }
    for v in &d.volumes {
        w.add_node(node_for(
            &v.transform,
            &format!("{}volume {}", v.scope, short_name(&v.type_path)),
            json!({"volume": true, "type": v.type_path, "scope": v.scope, "object": v.object,
                   "dimensions": v.dimensions, "length": v.length, "width": v.width,
                   "height": v.height, "radius": v.radius}),
        ));
    }
    let probes = if opts.probes {
        export_probes(game, d, out_dir, &mut report)
    } else {
        Vec::new()
    };
    let mut page_list: Vec<&PageFiles> = lm_pages.values().collect();
    page_list.sort_by_key(|p| p.index);
    w.set_scene_extras(json!({
        "level": d.path,
        "environments": d.environments.iter().map(|e| json!({
            "kind": format!("{:?}", e.kind), "type": e.type_path, "scope": e.scope,
            "position": e.position, "ambientLight": e.ambient_light, "fogColor": e.fog_color,
            "fogDensity": e.fog_density, "overrideLevelFog": e.override_level_fog,
            "reflectionMap": e.reflection_map, "backdrop": e.backdrop,
            "lighting": e.lighting.to_json(),
        })).collect::<Vec<_>>(),
        "lightmaps": page_list.iter().map(|p| json!({
            "page": p.index, "clr": p.clr_file, "clrSize": p.clr_size,
            "dir": p.dir_file, "dirSize": p.dir_size,
            "clrPath": p.clr_path, "dirPath": p.dir_path,
        })).collect::<Vec<_>>(),
        "lightmapFormat": lightmap_format_doc(),
        "reflectionProbes": probes,
    }));
    report.buffer_bytes = w.buffer_len();
    w.finish(name, out_dir)?;
    Ok(report)
}

// ---------------------------------------------------------------------------
// Collision

impl LevelDescription {
    /// Reads the PhysX shapes of every model a visible instance uses (one
    /// model at a time) into [`LevelDescription::collision`]. Returns the
    /// models that failed, with the reason.
    pub fn load_collision(&mut self, game: &Path) -> Result<Vec<(String, String)>> {
        let mut head = Package::open_in(game, "H.Misc")?.context("no H.Misc")?;
        let mut body = Package::open_in(game, "B.Misc")?;
        let mut used = vec![false; self.models.len()];
        for i in &self.instances {
            if !i.hidden {
                used[i.model] = true;
            }
        }
        let mut failures = Vec::new();
        self.collision.clear();
        for (m, path) in self.models.iter().enumerate() {
            if !used[m] {
                self.collision.push(None);
                continue;
            }
            let result = (|| -> Result<Vec<Shape>> {
                let raw = head
                    .read_path(path)?
                    .with_context(|| format!("{path} is not in H.Misc"))?;
                let header = ModelHeader::parse(&raw)?;
                if header
                    .physx_shapes
                    .iter()
                    .all(|s| s.len == 0 && s.kind == 0)
                {
                    return Ok(Vec::new());
                }
                let b = match body.as_mut() {
                    Some(p) => p.read_path(path)?.unwrap_or_default(),
                    None => Vec::new(),
                };
                read_shapes(&header.physx_shapes, &b)
            })();
            match result {
                Ok(shapes) => self.collision.push(Some(Rc::new(shapes))),
                Err(e) => {
                    failures.push((path.clone(), format!("{e:#}")));
                    self.collision.push(None);
                }
            }
        }
        Ok(failures)
    }
}

fn quat_rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let t = Transform {
        translation: [0.0; 3],
        rotation: q,
        scale: [1.0; 3],
    }
    .matrix();
    std::array::from_fn(|i| (0..3).map(|j| t[i][j] * v[j]).sum())
}

/// Triangles for one shape in the model's space (game handedness).
fn shape_triangles(s: &Shape) -> Option<(Vec<[f32; 3]>, Vec<u32>)> {
    let place = |p: [f32; 3]| -> [f32; 3] {
        let r = quat_rotate(s.rotation, p);
        std::array::from_fn(|k| r[k] + s.translation[k])
    };
    match &s.collider {
        Collider::Empty => None,
        Collider::Triangles(m) => Some((
            m.vertices.iter().map(|&p| place(p)).collect(),
            m.indices.clone(),
        )),
        Collider::Convex(m) => {
            let mut idx = Vec::new();
            for (corners, _) in &m.polygons {
                for k in 1..corners.len().saturating_sub(1) {
                    idx.extend([corners[0], corners[k], corners[k + 1]]);
                }
            }
            Some((m.vertices.iter().map(|&p| place(p)).collect(), idx))
        }
        Collider::Box { half_extents: h } => {
            let verts: Vec<[f32; 3]> = (0..8)
                .map(|i| {
                    place([
                        if i & 1 == 0 { -h[0] } else { h[0] },
                        if i & 2 == 0 { -h[1] } else { h[1] },
                        if i & 4 == 0 { -h[2] } else { h[2] },
                    ])
                })
                .collect();
            let quads = [
                [0, 1, 3, 2],
                [4, 6, 7, 5],
                [0, 2, 6, 4],
                [1, 5, 7, 3],
                [0, 4, 5, 1],
                [2, 3, 7, 6],
            ];
            let idx = quads
                .iter()
                .flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]])
                .collect();
            Some((verts, idx))
        }
        Collider::Sphere { radius } => Some(crate::collision::uv_sphere(*radius, 0.0, &place)),
        Collider::Capsule {
            radius,
            half_height,
        } => Some(crate::collision::uv_sphere(*radius, *half_height, &place)),
    }
}

/// Writes `<name>_collision.gltf`: every model's shapes once, a node per
/// instance placing them. Returns (shapes drawn, nodes).
pub fn export_collision(
    d: &LevelDescription,
    out_dir: &Path,
    name: &str,
) -> Result<(usize, usize)> {
    let mut w = LevelWriter::new();
    let mut mesh_of: Vec<Option<usize>> = vec![None; d.models.len()];
    let (mut shapes_drawn, mut nodes) = (0, 0);
    for inst in &d.instances {
        if inst.hidden {
            continue;
        }
        let Some(shapes) = d.collision_shapes(inst.model) else {
            continue;
        };
        let mesh = match mesh_of[inst.model] {
            Some(m) => m,
            None => {
                // All shapes of the model merged into one mesh
                let (mut verts, mut idx) = (Vec::new(), Vec::new());
                for s in shapes {
                    if let Some((v, i)) = shape_triangles(s) {
                        let base = verts.len() as u32;
                        verts.extend(v.into_iter().map(|p| [-p[0], p[1], p[2]]));
                        // Mirroring x flips the winding
                        idx.extend(
                            i.as_chunks::<3>()
                                .0
                                .iter()
                                .flat_map(|t| [t[0] + base, t[2] + base, t[1] + base]),
                        );
                        shapes_drawn += 1;
                    }
                }
                if idx.is_empty() {
                    continue;
                }
                let m = w.add_simple_mesh(short_name(&d.models[inst.model]), &verts, &idx);
                mesh_of[inst.model] = Some(m);
                m
            }
        };
        let mut node = node_for(
            &inst.transform,
            &format!("{}{}", inst.scope, short_name(&d.models[inst.model])),
            json!({"object": inst.object, "model": d.models[inst.model]}),
        );
        node["mesh"] = json!(mesh);
        w.add_node(node);
        nodes += 1;
    }
    w.finish(&format!("{name}_collision"), out_dir)?;
    Ok((shapes_drawn, nodes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrinks_by_box_filter() {
        let mut img = Image::new(4, 2);
        for (i, px) in img.rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            px.copy_from_slice(&[(i * 10) as u8, 100, 0, 255]);
        }
        let s = shrink(&img, 2);
        assert_eq!((s.width, s.height), (2, 1));
        // Pixels 0, 1, 4, 5 average to (0 + 10 + 40 + 50) / 4 = 25
        assert_eq!(s.rgba[..4], [25, 100, 0, 255]);
        // Already small enough: unchanged
        assert_eq!(shrink(&img, 4).width, 4);
    }

    #[test]
    fn matrices_are_column_major() {
        let m = Transform {
            translation: [1.0, 2.0, 3.0],
            ..Transform::IDENTITY
        }
        .matrix();
        let c = column_major(m);
        assert_eq!(&c[12..], [1.0, 2.0, 3.0, 1.0]);
        assert_eq!(c[0], 1.0);
    }

    #[test]
    fn collision_shapes_become_triangles() {
        let boxed = Shape {
            translation: [10.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            collider: Collider::Box {
                half_extents: [1.0, 2.0, 3.0],
            },
        };
        let (v, i) = shape_triangles(&boxed).unwrap();
        assert_eq!((v.len(), i.len()), (8, 36));
        assert!(v
            .iter()
            .all(|p| (9.0..=11.0).contains(&p[0]) && p[1].abs() <= 2.0 && p[2].abs() <= 3.0));
        assert!(i.iter().all(|&k| (k as usize) < v.len()));
        // Rotating a quarter turn about z swaps the x and y extents
        let q = std::f32::consts::FRAC_1_SQRT_2;
        let turned = Shape {
            rotation: [0.0, 0.0, q, q],
            translation: [0.0; 3],
            ..boxed.clone()
        };
        let (v, _) = shape_triangles(&turned).unwrap();
        let max_x = v.iter().map(|p| p[0]).fold(f32::MIN, f32::max);
        assert!((max_x - 2.0).abs() < 1e-5);
        let sphere = Shape {
            collider: Collider::Sphere { radius: 2.0 },
            ..boxed.clone()
        };
        let (v, i) = shape_triangles(&sphere).unwrap();
        assert!(v.iter().all(
            |p| (((p[0] - 10.0).powi(2) + p[1].powi(2) + p[2].powi(2)).sqrt() - 2.0).abs() < 1e-4
        ));
        assert!(i.iter().all(|&k| (k as usize) < v.len()));
        let empty = Shape {
            collider: Collider::Empty,
            ..boxed
        };
        assert!(shape_triangles(&empty).is_none());
    }
}
