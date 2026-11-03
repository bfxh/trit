use std::path::{Path, PathBuf};

pub(crate) fn arg(argv: &[String], key: &str) -> Option<String> {
    let i = argv.iter().position(|a| a == key)?;
    argv.get(i + 1).cloned()
}

pub(crate) fn frame_path(dir: &Path, sub: &str, n: usize, ext: &str) -> PathBuf {
    dir.join(sub).join(format!("{n:06}.{ext}"))
}

pub(crate) fn usage() -> ! {
    eprintln!(
        "用法:
  parts-3d cloud --seg <npy> --depth <npy> --meta <json> --out <ply>
  parts-3d infer --model <tpm> --scene <dir> --frame N --manifest <json> --out <ply> [--depth <npy>]
  parts-3d eval --model <tpm> --scene <dir> --manifest <json> [--limit N] [--voxel 0.01] [--min-vox 30]
  parts-3d mesh --seg <npy> --depth <npy> --meta <json> --out <ply> [--cell 0.01] [--levels 0,2,4]
  parts-3d fuse --scene <dir> --frames 0,1,2,3 [--eval 4,5,6,7] --out <ply> [--cell 0.01] [--min-votes 1] [--voxel 0.01] [--levels 0,2]
  parts-3d splat --scene <dir> --frames 0,2,4,6 --eval 1,3,5,7 [--cells 0.04,0.02,0.01] [--scale-k 2] [--alpha 0.6]
  parts-3d occ --root <renders 根> --scenes <scenes 根> --train 80,81 --eval 84,85 [--cell 0.01] [--steps 20000] [--selfsup] [--views N] [--tau 0.01]
  parts-3d align --scene <dir> --frames 0,1,2,3,4,5,6,7 [--cell 0.01] [--tau 0.01] [--fill]
  parts-3d obs-split --scene <dir> --frames 0,1,2,3,4,5,6,7 --out <ply> [--cell 0.01] [--tau 0.01]
  parts-3d mesh-tex --scene <dir> --frames 0,1,2,3,4,5,6,7 --out <ply> [--cell 0.01] [--tau 0.01]
  parts-3d tex --kind wood|marble|rust|grid --size 256 --out <ppm>
  parts-3d tex-prior [--iters 30000] [--patch 3|7] [--cond] [--hid 16]
  parts-3d tex-conv  [--iters 20000] [--hidden 8,8] [--cond] [--size 64] [--adam] [--batch K] [--bn]
  parts-3d tex-quilt <源.png> [--out out.ppm] [--size 128] [--patch 12] [--overlap 3] [--seed 1]
  parts-3d tex-stylize <in.png> [--preset cel|oil|epic --strength 0..1] [--contrast K] [--levels N] [--warmth T] [--sat S] [--stroke R] --out o.png
  parts-3d tex-gen   [--iters 200000] [--steps-t 50] [--size 64] [--hid 32] [--out <dir>]
                     [--conv 8,8] [--batch 4] [--adam] [--eps|--v] [--eps-space] [--diag]
                     [--dil N] [--save w.tpc] [--load w.tpc] [--holdout-names A,B]
  parts-3d look <图或目录>… [--out features.csv]"
    );
    std::process::exit(2);
}

pub(crate) fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}
