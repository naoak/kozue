# kozue IR 拡張ロードマップ

## 目的

Mermaid / PlantUML の対応範囲を拡大しながら、次の要件を維持する。

- 同一入力から常に byte-identical な出力を生成する
- frontend → semantic IR → layout → renderer の境界を維持する
- source 固有の構文を共通 IR に無理に持ち込まない
- draw.io / Excalidraw / PPTX などへ意味情報を失わずに渡す
- serialized IR の互換性を明示的に管理する

すべての diagram を万能な graph に平坦化しない。`Diagram` は domain 別の
variant を維持し、ID、annotation、container、style など、本当に共有できる
概念だけを共通型として抽出する。

## 全体マイルストーン

### M1: Versioned IR document

状態: **実装済み、コミット済み**

- `IrDocument` と数値 wire version `IrSchemaVersion::V1` を追加
- `CURRENT_IR_SCHEMA_VERSION` を追加（M1 時点では V1、M2 で V2 へ更新）
- diagram name、title、description、accessibility metadata の受け皿を追加
- 決定的な `BTreeMap` ベースの namespaced `Extensions` を追加
- 未知 schema version を deserialize 時に拒否
- 新設 public 型を `#[non_exhaustive]` にして将来の Rust API 拡張に備える
- native DSL / Mermaid / PlantUML に `parse_document` を追加
- 既存 `parse(source) -> Diagram` と既存 `Diagram` wire 表現を維持
- native DSL 全5種と PlantUML の diagram name を保持
- Mermaid の name は V1 では `None`

V1 では `Extensions` の変更 API を公開しない。shape、relation kind、note、
fragment などの core semantics を extension に格納してはならない。

### M2: Stable element identity and annotations

状態: **実装済み、コミット済み**

- transparent newtype `ElementId` を導入
- 既存5図の named element、ordered map key、`from` / `to`、
  `Endpoint::State` を `ElementId` へ移行
- 対応する `SemanticLayout` の ID と endpoint も `ElementId` へ移行
- raw parser AST、diagnostic、Scene group 名、renderer 固有 ID は `String` のまま維持
- `IrDocument.annotations: Vec<Annotation>` を追加し、宣言順を保持
- diagram / single element / multiple elements を annotation target として型付け
- note、link、tooltip、stereotype、tag の共通 annotation payload を追加
- schema V2 を追加し、V1 document を空 annotations の V2 へ lossless upgrade
- M2 時点の serialize は V2（M3a1 で V3、M3a2a-I で V4、M3a2a-II で V5、M3a2b で V6、
  M3a3 で V7 へ更新）、未知 version、必須 field 欠落、未知 nested field を拒否
- bare `Diagram` の既存 JSON wire 表現と renderer 出力を維持

M2 では `PortId`、source provenance sidecar、style token、無名 relation / message /
transition 自体の ID、annotation の frontend 構文対応を延期した。annotation ID の重複、
dangling target、空の multi-element target の semantic validation も、実際に frontend が
annotation を生成するマイルストーンで追加する。

### M3x0: Exchange exporter contract bridge

状態: **実装済み**

- `LayoutOutput::export_input(&Diagram)` が diagram / scene / semantic layout を借用し、
  top-level variant、5 domain の identity/order/index/semantic parity、有限かつ非負の geometry を検証
- `ExportInput` は clone を持たず、private field と getter のみを公開
- draw.io / Excalidraw / PowerPoint に strict `render_export` API を追加し、legacy `render` と同一 bytes を維持
- CLI の exchange 3形式だけを strict contract 経由に変更。SVG / terminal / PNG / DOT の入力境界は維持
- dangling graph/class/ER relation、dangling sequence participant、illegal state endpoint と
  future enum fallback を layout error に変更
- IR schema と既存 golden bytes は変更しない

### M3: Existing diagram semantics

状態: **M3a3（subgraph / container）実装済み**

既存5種を frontend ごとの最小 subset から、意味を保持できる IR へ拡張する。

1. Graph / Flowchart
   - **M3a1 実装済み**: Down / Right / Up / Left
     - native DSL `direction up|left`
     - Mermaid `BT` / `RL`
     - graph / class layout の主軸反転
     - DOT `rankdir=TB/LR/BT/RL`
     - schema V3 と V1 / V2 document migration
   - **M3a2a-I 実装済み**: legacy Default / Rectangle / RoundedRectangle
     - native DSL `shape rectangle|rounded`
     - Mermaid bare / `[label]` / `(label)` の shape 保持
     - layout と全 backend への shape 伝播
     - schema V4 と V1 / V2 / V3 document migration
   - **M3a2a-II 実装済み**: Circle / Diamond
     - native DSL `shape circle|diamond`
     - Mermaid `((label))` / `{label}` と明示宣言の last-wins 更新規則
     - shape 固有の sizing、Scene path、edge endpoint clipping
     - SVG / PNG / terminal / draw.io / DOT / Excalidraw / PPTX への shape 伝播
     - schema V5 と V1 / V2 / V3 / V4 document migration
   - **M3a2b 実装済み**: edge semantics / presentation
     - schema V6。旧 `Edge::new(..., ArrowType)` と legacy `arrow` wire bytes は維持
     - `Edge` に `from_arrow`（source marker）、`line: Solid|Dashed|Dotted`、
       `weight: Normal|Thick` を追加し、directed / undirected / bidirectional を
       型付け
     - native DSL: `a -> b`（directed）/ `a --- b`（undirected）/
       `a <-> b`（bidirectional）に加え、`: "label"` の前に置く
       `line solid|dashed|dotted` / `weight normal|thick` modifier、formatter の
       canonical 出力
     - Mermaid: `-.->` / `-.-` / `==>` / `===` / `<-->` と `|label|` pipe-label
       subset を追加
     - source 端 arrowhead の layout retraction（bidirectional の始点側矢印分だけ
       経路を後退させる）
     - Scene path、SVG / PNG / terminal stroke、DOT (`dir` / `style` /
       `penwidth`)、draw.io (`startArrow` / `dashed` / `dashPattern` /
       `strokeWidth`)、Excalidraw (`strokeStyle` / `strokeWidth` /
       `startArrowhead`)、PPTX (`prstDash` / `w` / `headEnd`) への全 backend 伝播
     - M3x0 の exchange exporter contract を拡張し、新規 edge field も検証対象に含める
     - 既存 golden 差分0、新規 `edge_presentation` golden のみ追加
   - **M3a3 実装済み**: subgraph / container
     - schema V7。旧 document は空 `containers: []` へ lossless upgrade、V1-V6 で
       非空 `containers` を明示的に拒否
     - `Container { id, label, members, children }` の木構造を `GraphDiagram`
       に追加。`members` は flat `nodes` map の id への参照方式（node 本体を
       container 側に複製しない）で、root-level container は `containers`
       に、入れ子 container は親の `children` に宣言順で保持
     - native DSL: `subgraph id [: "label"] { <node decls + nested subgraph> }`。
       body には node 宣言と入れ子 subgraph のみ許容し、空 subgraph、body 内
       edge 文、state / sequence など graph 以外での使用はすべて拒否
     - Mermaid: `subgraph id [Title]` / bare title / nested subgraph +
       `end`。node の first-mention（最初に出現した場所）を membership とする。
       per-subgraph `direction` は未対応（Partial）
     - layout は node 配置・edge routing を変えない naive bounding-box 方式
       （container 内の node group の bounding box に `CONTAINER_PAD` を
       足した矩形を描くだけ）。containment を考慮したレイアウト最適化は M4 で
       扱う
     - `SemanticLayout` に pre-order `containers: Vec<ContainerLayout>` を追加
     - 全 backend 伝播: SVG / PNG / terminal は破線の矩形＋左上ラベル文字列を
       node の背後に描画、DOT は native `subgraph cluster_N { label=...; }`
       の入れ子、draw.io / Excalidraw / PPTX は node と同じ座標系上に
       backdrop 方式（塗りなし破線の矩形 + 独立したラベルテキスト）で表現
     - M3x0 の exchange exporter contract を拡張し、container の parity /
       geometry も検証対象に含める
     - PlantUML は graph 用 frontend / parser が存在しないため対象外
       （既存5種の PlantUML は sequence / state / class / ER のみ）
     - 既存 golden 差分0、新規 `subgraph` / `mermaid_subgraph` golden のみ追加
   - **M3a2 次候補**:
     - M3a4 port
2. Sequence
   - participant kind
   - note、activation、create / destroy
   - divider、delay、reference
   - `loop` / `alt` / `opt` / `par` / `critical` / `break` の再帰 fragment
   - open / filled / cross / circle / async / bidirectional arrow
3. State
   - composite state と region の階層構造
   - Initial / Final / Choice / Fork / Join / History の typed pseudostate
   - state description と internal behavior
   - transition の trigger / guard / effect
4. Class
   - member を visibility / name / type / parameter / modifier に構造化
   - namespace / package containment
   - association / generalization / realization / dependency を意味型で保持
5. ER
   - key と cardinality の型付け
   - direction、constraint、index metadata
   - layout 後も属性を整形済み文字列へ潰さない

### M4: Layout and exchange exporter contract

状態: **未着手**

- 現在の lossy な `SemanticLayout` を見直す
- layout output を `ElementId -> Geometry` mapping に寄せる
- exchange exporter に元の `Diagram` と geometry の両方を渡す
- shape、container、port、annotation、structured member を exporter まで保持
- Scene primitive に paint、stroke、ellipse、image / icon、semantic role を追加
- 未知 primitive や未対応 semantic item の silent skip を禁止する

### M5: Shared new diagram families

状態: **未着手**

共通性と利用価値の高い順に、domain 固有 variant として追加する。

1. Use case / Requirement
2. Component / Deployment / Architecture
3. Activity / Swimlane
4. Mindmap / Tree / WBS
5. Timeline / Gantt / Chronology
6. Network / structured data

### M6: Charts and specialized diagrams

状態: **未着手**

- Pie、XY、Radar、Quadrant、Sankey、Venn などは専用 semantic model を持つ
- Packet、Kanban、GitGraph、Timing、EBNF / Regex も必要に応じて独立 variant とする
- Salt、Ditaa、Math などは後順位とし、opaque passthrough を採用する場合も
  deterministic / hermetic な入力だけを許可する

## Source-specific extensions

共通 IR に正規化しない候補:

- Mermaid frontmatter、theme variables、renderer 指定、raw CSS、`classDef`
- PlantUML `skinparam`、`<style>`、Creole 原文、layout engine / pragma
- exact source round-trip 用の delimiter、quote、spelling
- preprocessor macro / include の定義情報

PlantUML の remote `!include`、`%load_json`、`%now`、Gantt の `today` などは
決定性要件と衝突する。preprocessor は IR の外に置き、外部入力はデフォルトで
禁止する。許可する場合は content hash と固定 evaluation context を要求する。

## Contract tests

各マイルストーンで以下を追加・維持する。

- frontend 間の等価入力が意味的に同一の IR になること
- IR schema fixture、round-trip、version migration / rejection
- 同一データの serialization が byte-identical であること
- 全 semantic element に対応 geometry が存在すること
- 対応 backend の golden test
- unsupported feature が silent downgrade / silent drop されないこと
- golden 更新時は `UPDATE_GOLDEN=1 cargo test` 後に必ず差分を確認すること

## M1 / M2 の検証状況

- `cargo fmt --check`: 成功
- `cargo check --workspace`: 成功
- `cargo test --workspace --no-run`: 成功
- `cargo test --workspace --exclude kozue-cli`: 成功
- `kozue-ir` の schema / migration / typed ID tests 9件: 成功
- CLI integration: 69 / 69 成功
- `cargo clippy --workspace --all-targets -- -D warnings`: 成功
- `git diff --check`: 成功

`drawio_class_er_goldens_match` で残っていた class / ER の draw.io golden は、
`value` 属性内の HTML を正しく XML escape した renderer 出力に合わせて更新済み。
独立レビューの指摘5件はすべて修正後に再レビュー済みで、blocking finding は
残っていない。

## M3a1 の検証状況

- `cargo test --workspace`: 成功
- CLI integration: 70 / 70 成功
- `kozue-ir` の schema / migration tests 12件: 成功
- graph / class の4方向、可変寸法、dummy route、bounds、決定性 tests: 成功
- `cargo clippy --workspace --all-targets -- -D warnings`: 成功
- `cargo fmt --check`: 成功
- `git diff --check`: 成功
- 独立レビューと2回の修正確認後、blocking findingなし

## M3a2a-I の検証状況

- schema V4 migration と旧 schema の明示 shape 拒否 tests: 成功
- native / Mermaid shape 等価性、formatter、unsupported shape tests: 成功
- layout kind 伝播、corner geometry、route 不変 tests: 成功
- SVG / PNG / terminal / draw.io / DOT / Excalidraw / PPTX mapping tests: 成功
- Mermaid `[label]` の Rectangle 化に伴う SVG golden のみ更新
- `cargo test --workspace` と workspace Clippy: 成功

## M3a2a-II の検証状況

- schema V5 migration と V1-V4 の node kind 互換性 matrix tests: 成功
- native / Mermaid の Circle / Diamond 構文、明示宣言更新規則、formatter tests: 成功
- sizing、固定 path 順序、解析的 endpoint clipping tests: 成功
- 全 backend mapping integration tests と `node_shapes` goldens: 成功

## M3a2b の検証状況

- schema V6 migration と V1-V5 document の互換性 tests: 成功
- native DSL の `->` / `---` / `<->` と `line` / `weight` modifier、formatter
  canonical 出力 tests: 成功
- Mermaid `-.->` / `-.-` / `==>` / `===` / `<-->` と pipe-label subset tests: 成功
- `native_and_mermaid_edge_presentation_produce_equivalent_ir`: 成功
  （Mermaid に plain dashed graph edge token が無いため、dashed の等価性検証は
  `kozue-dsl` / `kozue-mermaid` 側の単体テストで別途担保）
- source 端 arrowhead の layout retraction tests: 成功
- SVG / PNG / terminal / DOT / draw.io / Excalidraw / PPTX の全 backend mapping
  integration test (`edge_presentation_maps_across_all_backends`) と新規
  `edge_presentation` golden（`.svg` / `.txt` / `.png` / `.dot` / `.drawio` /
  `.excalidraw` / `.pptx`）: 成功。既存 golden の bytes は変更なし
- PNG の dashed-only / dotted-only / thick-only 3 variant が決定的に異なる bytes
  を生成することを確認
- M3x0 exchange exporter contract の拡張分を含む `strict_exchange_export_matches_legacy_bytes_for_all_domains_and_is_deterministic`: 成功
- `cargo fmt --check`: 成功
- `cargo check --workspace`: 成功
- `cargo test --workspace`（`UPDATE_GOLDEN=1` なし）: 成功。CLI integration 75 / 75
  成功（node_shapes 相当のケース数 + 新規1件）
- `cargo clippy --workspace --all-targets -- -D warnings`: 成功
- `git diff --check`: 成功
- 独立レビュー: blocking / major finding なし。minor 2件のうち、実質を検証しない
  span test は削除、SVG / PNG の `Dotted` を明示 arm 化してコメントを future
  variant 専用に修正。`line` / `weight` modifier が改行を跨いで次行の同名 ident を
  吸収しうる件は、既存 `shape` modifier と同一の文法特性として既知事項扱い
- `Edge` の新 field は `Node.kind` 追加時と同じく required（schema envelope 単位の
  互換管理。inner struct の旧 JSON bytes 単位ではない）

## M3a3 の検証状況

- schema V7 migration と V1-V6 document の非空 `containers` 明示拒否 tests: 成功
- native DSL の `subgraph id [: "label"] { ... }`、nested subgraph、
  空 subgraph / body 内 edge / subgraph id と node id の衝突 / state・sequence
  での使用禁止 tests: 成功
- Mermaid `subgraph` / `end`、bare title / `[Title]`、nested subgraph、
  first-mention membership tests: 成功
- `native_and_mermaid_subgraphs_produce_equivalent_ir` /
  `native_and_mermaid_nested_subgraphs_produce_equivalent_ir`: 成功
- layout の pre-order `SemanticLayout.containers`、bounding-box +
  `CONTAINER_PAD` geometry、既存 node 配置・edge routing 不変 tests: 成功
- SVG / PNG / terminal / DOT / draw.io / Excalidraw / PPTX の全 backend mapping
  integration test (`subgraphs_map_across_all_backends`) と新規 `subgraph`
  golden（`.kzd` / `.svg` / `.txt` / `.png` / `.dot` / `.drawio` /
  `.excalidraw` / `.pptx`）、新規 `mermaid_subgraph` golden（`.mmd` / `.svg`）:
  成功。既存 golden の bytes は変更なし
- 目視確認: SVG は破線の container 矩形が node 矩形の手前（描画順で背後）に
  出力され、ラベル付き container は左上にラベル文字列、入れ子 container
  （`inner`）は親 container（`right`）の矩形内に収まっていることを確認。
  DOT は `subgraph cluster_0` / `cluster_1` に `cluster_2` が入れ子で
  含まれ、ラベル付きのみ `label=` を持つことを確認。draw.io は `c0`/`c1`/`c2`
  の `dashed=1` backdrop セルが `n0`-`n4` の node セルより前に出力されている
  ことを確認。Excalidraw は `dashed` の rectangle 要素と、ラベル付き
  container にのみ対応する自由テキスト要素（`c0-label` / `c2-label`）が
  node 要素より前に出力されていることを確認。PPTX は `Container N` という
  name を持つ no-fill 矩形 shape が `prstDash val="dash"` を持ち、
  `Node N` shape より前に出力されていることを確認
- M3x0 exchange exporter contract の拡張分を含む
  `strict_exchange_export_matches_legacy_bytes_for_all_domains_and_is_deterministic`:
  成功
- `cargo fmt --check`: 成功
- `cargo check --workspace`: 成功
- `cargo test --workspace`（`UPDATE_GOLDEN=1` なし）: 成功
- `cargo clippy --workspace --all-targets -- -D warnings`: 成功
- `git diff --check`: 成功
- `UPDATE_GOLDEN=1 cargo test` 実行時、並列テスト起動直後に新規 golden
  ファイルがまだ書き出されていないタイミングで `excalidraw_goldens_are_well_formed_json`
  / `pptx_goldens_are_well_formed_zip` が一過性に失敗するのを確認（既知の
  並列生成レース）。再実行後は全件成功
- 独立レビュー: blocking / major finding なし。minor 2件に対応 —
  subgraph 内の `direction` 行は方向 token（LR/RL/TB/BT/TD）が続く場合のみ
  per-subgraph direction override として拒否し、`direction` という名の node は
  subgraph 内外で同一に解釈されるよう修正（テストで固定化）。IR が空 container
  を deserialize 時に再検証しない点は `Container` の doc comment に明記
  （frontend が非空を保証し、layout は防御的に degenerate box を生成）。
  nit 対応として、自明な比較しかしていなかった DOT の byte-compat test を
  cluster 不在の検証に整理

## 再開時の確認事項

1. M3a3 subgraph / container は実装・検証済み（未コミット。コミット後に
   clean baseline のハッシュをここへ追記する）
2. 次は M3a4 port の詳細設計・実装から開始する
3. 実装後は別担当の独立レビューと root 総合レビューを行い、既存 golden 差分0を確認する
4. Graph 完了後は Sequence -> State -> Class -> ER の順で M3 を完了する
