pub(super) const USAGE: &str = "\
trit-wspace <command>
  read <artifact|-> [--out <path>]
  verify <artifact|-> --oracle <json|->  capability <artifact|-> --oracle <json|-> [--labels <json|->] [--max-params N] [--top K] [--mode flip|zero] [--layer N] [--row N]  scan <artifact|-> --oracle <json|-> [--labels <json|->] [--max-params N] [--sample-budget N] [--refine K] [--top K] [--mode flip|zero] [--layer N] [--row N] [--limit N]
  task-merge <base|-> <tuned|->... [--aligned] [--weight W[,W...]] [--trim K|--trim-margin M] [--dare-rate R|--dare-per-task R] [--dare-seed N] [--out <path>]
  script <artifact|-> --script <json|-> --oracle <json|-> [--out <path>]
  certify <artifact|-> (--keep <oracle|-> | --keep-samples N [--seed S]) (--patch <json|-> | --edited <artifact|->) [--edit <oracle|->] [--allow N] [--limit N] [--strict] [--out <path>] [--certificate <path>]
  patch <artifact|-> --patch <json|-> [--oracle <json|->] [--require-before] [--require-after] [--log <path>] [--out <path>]
  apply-candidate <artifact|-> --candidate <json|-> [--oracle <json|->] [--require-before] [--require-after] [--log <path>] [--out <path>]
  rebase <target|-> --base <base|-> --patch <json|-> [--skip-conflicts] [--oracle <json|->] [--require-before] [--require-after] [--out <path>]
  transplant <dest|-> --from <source|-> --layer N --rows i,j [--oracle <json|->] [--require-before] [--require-after] [--out <path>] | project <artifact|-> --basis <artifact> --out <path> [--layer N] | diff <a|-> <b|->
  dare-drop <base|-> --tuned <tuned|-> --rate R [--seed N] [--by-row] [--oracle <json|->] [--require-before] [--require-after] [--out <path>]
  repair <artifact|-> --oracle <json|-> [--rows L:R,L:R | --trits L:R:C:T,...] [--row-ops] [--keep <oracle|->] [--max-edits N] [--beam-width N] [--exhaustive-states N] [--out <path>]
  propose <artifact|-> --oracle <json|-> [--limit N] [--depth 1|2|3] [--pairs] [--max-pairs N] [--max-triples N]
  train-proposer <artifact|-> --oracle <json|-> --out <model.json> [--epochs N] [--lr R] [--holdout R] | propose-learned <artifact|-> --oracle <json|-> --model <model.json> [--limit N] [--verify]
  propose-gradient <artifact|-> --oracle <json|-> [--limit N] [--verify]
  explain <artifact|-> --oracle <json|-> --sample N [--top K]  inventory <artifact|->  attribute <artifact|-> --oracle <json|-> [--mode flip|zero] [--layer N] [--row N] [--max-params N] [--top K]  impact <artifact|-> --oracle <json|-> (--edits <json|-> | --group row|layer --layer N [--row N] [--mode flip|zero])  repair-exhaustive <artifact|-> --oracle <json|-> [--max-edits 1|2] [--max-pairs N]
  probe <artifact|-> --oracle <json|-> [--budget N] [--top K] [--adaptive]  diagnose <artifact|-> --oracle <json|-> [--budget N] [--top K] [--max-edits N] [--beam-width N]
  localize <artifact|-> --oracle <json|-> [--rows K] [--row-budget N] [--max-trits N] [--top-trits K]
  audit <log|-> [--artifact <initial|->]
  shard <log|-> --size N --out <dir>
  audit-shards <dir> [--artifact <initial|->] [--out <path>] [--manifest <path>] [--resume <checkpoint>] [--checkpoint-every N --checkpoint-dir D]
  replay <log|-> --artifact <initial|-> [--out <path>]
  merge <a|-> <b|-> [--aligned] [--out <path>]
  list-safetensors <file>
  import-safetensors <file> --tensor <name> [--out <path>]
  export-safetensors <artifact|-> --out <path> [--tensor <name>]
  import-safetensors-dir <file> --out <dir>
  export-safetensors-dir <dir> --out <file>
  import-gguf <file> --out <dir>
  export-gguf <dir> --out <file>
  index-artifact <artifact.wsp2> --out <idx>
  read-block <artifact.wsp2> --index <idx> --layer N [--row-start R --rows K] [--verify] [--out <path>]
  split-artifact <artifact|-> --rows N --out <dir>
  join-artifact <dir> --out <path>
  pack-artifact <artifact|-> --out <bin> | unpack-artifact <bin> --out <path>
  mcp";
