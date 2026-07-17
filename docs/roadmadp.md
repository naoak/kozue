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
  M3a3 で V7、M3a4 で V8 へ更新）、未知 version、必須 field 欠落、未知 nested field を拒否
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

状態: **M3b3（note ＋ SemanticLayout item 列一般化、schema V11）実装済み**

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
   - **M3a4 実装済み**: port
     - schema V8。旧 document は port なし edge へ lossless upgrade、V2-V7 で
       非 None port を明示拒否（V1 は wire arm が先に拒否）
     - `Edge.from_port` / `to_port: Option<Port>`（North / East / South / West の
       4 基本方位のみ。`Port` は `#[non_exhaustive]` で、ordinal / center /
       offset port は後続 sub-milestone に延期）
     - native DSL: `a.north -> b.south`。`.` は端点に密着（`a . north` は
       syntax error）、port 語は予約語ではなく node id としても有効、
       unknown port と graph 以外（state / sequence）での使用は明示エラー、
       formatter は full name で canonical 出力
     - Mermaid: port / compass 構文が存在しないため対象外（Partial）。
       port 等価性は kozue-dsl 単体テストで担保
     - layout は cardinal port を既存 shape clip（`clip_to_shape`）への
       軸方向 unit ray 再利用で side 中点 / 頂点へ snap。routing・node 配置は
       不変で、port なし edge は旧コードパスをそのまま通る
     - DOT は native compass（`"a":e -> "b":w`）、draw.io は
       `exitX/exitY/entryX/entryY`（source exit → target entry の固定順）、
       SVG / PNG / terminal / Excalidraw / PPTX は snap 済み route 経由で
       コード変更なし。future `Port` variant は全 match で明示拒否
     - M3x0 exchange exporter contract に port parity と future variant 拒否を追加
     - 既存 golden 差分0、新規 `ports` golden のみ追加
2. Sequence
   - **M3b1 実装済み**: participant kind
     - schema V9。旧 document は participant kind なし（Default）へ lossless
       upgrade、V2-V8 で非 Default kind を明示拒否（V1 は wire arm が先に拒否）
     - `ParticipantKind`（Default / Actor / Boundary / Control / Entity /
       Database / Collections / Queue、`#[non_exhaustive]`）を追加し、
       `Participant.kind` を新設。`participant_kind_supported_in` gate と
       `diagram_supported_in` の Sequence arm で検査
     - native DSL: `actor` / `boundary` / `control` / `entity` / `database` /
       `collections` / `queue <id>[: "label"]` を宣言キーワードとして追加
       （`participant` は Default）。kind 語は予約語ではなく id にも使え、
       formatter は kind キーワードで canonical 出力（idempotent）
     - Mermaid: `actor X` を `ParticipantKind::Actor` に格上げ（従来 Partial）。
       他 kind は Mermaid に構文が無く対象外
     - PlantUML: 従来 Participant に潰していた actor / boundary / control /
       entity / database / collections / queue を kind 保持に格上げ
       （features 表を Partial → Supported）
     - layout / SemanticLayout: `ParticipantLayout.kind` を追加。非 Default は
       header 内ラベル上に `«kind»` guillemet 行を一様追加するだけの表現で、
       Default participant の geometry は完全不変（stick-figure 等の凝った
       描画は後続 polish 送り）。header 高さは非 Default のときのみ増加
     - 全 backend 伝播: SVG / PNG / terminal は Scene Text 経由、
       draw.io / Excalidraw / PPTX は header shape ラベルに stereotype 行を反映。
       Default 出力 bytes は不変。DOT は sequence 非対応（`UnsupportedDiagram`）
       を維持。future `ParticipantKind` variant は exchange contract の
       `validate_export_semantics` が明示拒否し、提示系パスは安全 fallback
     - M3x0 exchange exporter contract に participant kind parity と future
       variant 拒否を追加
     - 既存 golden 差分0、新規 `seq_participant_kinds`（`.kzd` / `.svg` /
       `.drawio` / `.excalidraw` / `.pptx`）と `mermaid_seq_actor`
       （`.mmd` / `.svg`）golden のみ追加
   - **M3b2 実装済み**: message arrow / async（open / filled / cross / circle /
     async / bidirectional）
     - schema V10。旧 document は arrow 表現可能な範囲へ lossless upgrade、
       V2-V9 で Open / Cross / Circle head または非 None tail を明示拒否
       （V1 は wire arm が先に拒否）
     - **共有 `ArrowType`（graph / class / ER 共用、Triangle / None）は変更せず**、
       sequence 専用に `MessageArrow`（None / Filled / Open / Cross / Circle、
       `#[non_exhaustive]`）を新設。`Message.arrow: ArrowType` を
       `head: MessageArrow` へ置換し `tail: MessageArrow`（始端）を追加。
       意味: Filled=同期、Open=非同期(async)、Cross=lost/destroy、
       Circle=found/circle-end、bidirectional = tail ≠ None。
       `Message::new(..., ArrowType)` は Triangle→Filled / None→None の互換
       コンストラクタとして維持、`Message::with_arrows` を追加
     - native DSL: 既存 graph edge の `line` / `weight` modifier と同流儀で
       message に `head` / `tail` modifier（`open|filled|cross|circle|none`）を
       追加。`head`/`tail` は予約語ではなく id にも使え、formatter は
       canonical 順（head→tail）で default（head filled / tail none）を省略出力
       （idempotent）。graph / state での誤用と未知値は明示エラー
     - Mermaid: `-)`→Open / `-x`→Cross / `<<->>`→bidirectional を追加。
       `->` / `-->` を実 Mermaid 準拠で None head に是正（従来 Partial の
       Triangle 化を解消。既存 golden は `->>` 系のみ使用のため差分0）
     - PlantUML: `->>`→Open 格上げ（従来 Partial）、`->x`→Cross / `->o`→Circle
       / `<->`→bidirectional を有効化（従来 unsupported error）。`->oscar` 等の
       誤認防止に arrow token の word-boundary ガードを追加
     - layout: `MessageLayout.head` / `tail`。対象端・始端に head 種別の glyph
       （Filled=塗り三角 / Open=V 字 / Cross=× / Circle=八角形近似）を描画し、
       glyph 分だけ両端で route を retract。default（Filled / None）では座標・
       Scene 生成が式レベルで既存一致し、直線 / self-loop の既存 golden bytes
       は不変。Circle は M4 の ellipse primitive までの暫定八角形近似（bounds
       に取り込み clip されない）
     - 全 backend 伝播: SVG / PNG / terminal は Scene 経由、draw.io は
       `startArrow` / `endArrow`（Filled→block/classic, Open→open, Circle→oval,
       Cross→cross）、Excalidraw は `startArrowhead` / `endArrowhead`
       （Filled→triangle, Open→arrow, Circle→dot, Cross→bar 近似）、PPTX は
       `headEnd` / `tailEnd`（Filled→triangle, Open→stealth, Circle→oval,
       Cross→diamond 近似）。exchange 系の Cross は損失近似だが doc-comment で
       明示（silent ではない）。DOT は sequence 非対応維持
     - M3x0 contract に head / tail parity と `MessageArrow` future variant 拒否を追加
     - 既存 golden 差分0、新規 `seq_message_arrows`（`.kzd` / `.svg` / `.txt` /
       `.png` / `.drawio` / `.excalidraw` / `.pptx`）・`mermaid_seq_arrows`
       （`.mmd` / `.svg`）・`plantuml_seq_arrows`（`.puml` / `.svg`）golden のみ追加
   - **M3b3 実装済み**: note ＋ SemanticLayout item 列一般化
     - schema V11。旧 document は note なし body へ lossless upgrade、V2-V10 で
       Note item を明示拒否（`sequence_note_supported_in` gate、V1 は wire arm が
       先に拒否）。全 `*_supported_in` gate に V11 を追加
     - `SequenceItem` に `Note(Note)` を追加。`Note { text, position, targets:
       Vec<ElementId> }`。位置は annotation 系 `NotePlacement`（Auto/Above/Below を
       含み sequence に不整合）とは別に **専用 `NotePosition`（LeftOf / RightOf /
       Over、`#[non_exhaustive]`）** を新設（`MessageArrow` を `ArrowType` と分けた
       のと同じ思想）。個数不変条件は LeftOf/RightOf==1・Over>=1（frontend と layout で検証）
     - **転換点**: `SemanticLayout` の sequence を「message 専用 1:1」から
       **item 列一般化**へ再設計。`SequenceLayout.messages: Vec<MessageLayout>` を
       `items: Vec<SequenceItemLayout>`（`Message(MessageLayout)` /
       `Note(NoteLayout)`）へ置換。exchange contract の
       `items.len()==messages.len()` 前提を **item-parity**（diagram.items[i] と
       layout.items[i] の variant 一致で zip 突合、交差は明示 mismatch）へ変更。
       後続 M3b7（fragment 再帰木）の布石
     - native DSL: `note over a[, b...] : "text"` / `note left of a : "text"` /
       `note right of a : "text"`。`note`/`over`/`left`/`right`/`of` は非予約語
       （lookahead 外れで他 alt へ fall-through）、item は宣言順で message と interleave、
       graph / state での使用は明示エラー、formatter は canonical 出力で idempotent
     - Mermaid: `Note over/left of/right of`（`Note`/`note` 両許容）を unsupported
       から格上げ、source 行順で items へ push
     - PlantUML: 単一行 `note over/left of/right of ... : text` を格上げ。
       複数行 `note ... end note` block / `hnote` / `rnote` は今回スコープ外で
       従来通り明示 unsupported error（silent drop なし。features 表も更新）
     - layout: note は 1 行占有し、UML dog-ear（折れ角）の Path outline + 中央 Text を
       Scene に描画。列幅は note 幅を既存の label-width / self-overhang 機構へ
       **加算的に**折り込む（note が無いとき col_x は現状と式レベルで一致 →
       既存 golden bytes 不変）。`NoteLayout { index, text, position, targets, rect,
       text_anchor }`。塗り無しのため lifeline は透過（M4 の paint primitive まで暫定）
     - 全 backend 伝播: SVG / PNG / terminal は Scene 経由でコード変更なし、
       draw.io は `shape=note` vertex、Excalidraw は rectangle 近似（UML note 図形が
       無いため doc-comment で損失明示）、PPTX は rect + text。DOT は sequence 非対応維持
     - contract に item-parity 突合・note geometry 検証・`NotePosition` future variant
       拒否を追加
     - 既存 golden 差分0、新規 `seq_notes`（`.kzd` / `.svg` / `.txt` / `.png` /
       `.drawio` / `.excalidraw` / `.pptx`）・`mermaid_seq_notes`（`.mmd` / `.svg`）・
       `plantuml_seq_notes`（`.puml` / `.svg`）golden のみ追加
   - activation、create / destroy
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

## M3a4 の検証状況

- schema V8 migration（V1-V7 の lossless upgrade）と V2-V7 document の非 None port
  明示拒否 tests: 成功。V1 は fixture に `annotations` field が無く wire arm が
  先に拒否するため port gate の検証ループから除外（テスト内コメントに明記）
- native DSL の全4 edge 演算子での port parse、modifier / label との併用、
  unknown port エラー、`a . north` / `a. north` の syntax error、port 語の
  node id 利用、state / sequence での port 拒否、formatter canonical 出力
  （idempotent）tests: 成功
- `[*]` 疑似状態遷移の端点に port を書いた場合は parse error になり silent drop
  は発生しない（`id_endpoint` は port 非対応のまま）
- layout の side 中点 / 頂点 snap（非正方形 box 含む Rectangle / Circle /
  Diamond の厳密座標比較）、port なし edge の route が旧 `clip_to_shape`
  経路と一致する回帰 tests: 成功。arrow retraction は route 端点由来のため
  変更不要であることを確認
- DOT compass suffix / port なし byte 不変、draw.io exit / entry 固定順 style /
  default byte 不変、contract の port parity mismatch 検出 tests: 成功
- 全 backend mapping integration test（`ports_map_across_all_backends`）と
  新規 `ports` golden（`.kzd` / `.svg` / `.txt` / `.png` / `.dot` / `.drawio` /
  `.excalidraw` / `.pptx`）: 成功。既存 golden の bytes は変更なし
  （`git diff --stat tests/golden` 空を確認）
- strict exchange export の ports 専用 test
  （`strict_exchange_export_matches_legacy_bytes_for_ports`）: 成功。
  既存 5 domain の determinism test は case list ごと不変
- 目視確認: SVG で `a.east` の端点が a 矩形の右辺中点 (93.70, 19.60) に
  snap、DOT golden に `"a":e -> "b":w` / `"b":s -> "c":n` と port なし
  `"a" -> "c"` が併存することを確認
- `UPDATE_GOLDEN=1 cargo test` 初回に M3a3 で記録済みの並列生成レース
  （excalidraw / pptx の well-formed test の一過性失敗）が再現、再実行で全件成功
- `cargo fmt --check` / `cargo check --workspace` / `cargo test --workspace`
  （`UPDATE_GOLDEN` なし）/ `cargo clippy --workspace --all-targets --
  -D warnings` / `git diff --check`: 成功
- 独立レビュー: blocking / major / minor finding なし（verdict: ship）。
  nit 1件（port gate 拒否テストの V1 ループが上流拒否により空振り）は
  V1 をループから外しコメントを追加して対応

## M3b の設計（サブマイルストーン分割）

Sequence の意味拡張は M3a と同じ流儀（schema+1 / 全 frontend + layout + 全
backend + contract 伝播 / 既存 golden 差分0 / 新規 golden のみ / silent drop
禁止 / DOT は sequence 非対応維持）で、直交・平坦なものを前、構造侵襲的な
ものを後に配置した 7 サブマイルストーンに分割する。

- **M3b1 participant kind（V9）**: 実装済み（上記）
- **M3b2 message arrow / async（V10）**: 実装済み（上記）。open / filled /
  cross / circle / async / bidirectional を `MessageArrow` head / tail で表現
- M3b3 note ＋ SemanticLayout item 列一般化（V11）: 初の非 Message item。
  ここで contract の `items.len()==messages.len()` 1:1 前提と `SequenceLayout`
  を、diagram.items 順に対応する統一 item 列へ一度だけ再設計する（geometry
  不変なので既存 golden bytes は不変）。これを先送りすると b4-b7 で毎回
  パッチが増える
- M3b4 divider / delay / reference（V12）: b3 の新 leaf item 土台を再利用
- M3b5 activation bar（V13）: 活性区間モデル（開始 / 終了・ネスト）を新設。
  `ActivationLayout`（lifeline 上の rect stack）追加、message 端点を bar 辺へ
- M3b6 create / destroy（V14）: 参加者ライフサイクル（生成で header 降下、
  破棄で × 終端）。lifeline y 範囲が可変に
- M3b7 fragment loop / alt / opt / par / critical / break（V15）: items が
  平坦 Vec → 再帰木。contract の zip を木 walk へ。最も侵襲的なので最後

## M3b1 の検証状況

- schema V9 migration（V1-V8 の lossless upgrade）と V2-V8 document の非 Default
  participant kind 明示拒否 tests（`non_default_participant_kinds_require_schema_v9`）:
  成功。V1 は fixture に `annotations` field が無く wire arm が先に拒否するため
  gate 検証ループから除外（M3a4 と同じ前例、コメントに明記）
- native DSL の 8 kind キーワード parse、kind 語の id 利用（`kind_keyword_usable_as_id`）、
  formatter idempotency（`fmt_participant_kinds_idempotent`）tests: 成功
- Mermaid `actor` の Actor 格上げ、PlantUML icon-variant keyword の kind 保持
  格上げ tests: 成功
- 等価性: `native_and_plantuml_participant_kinds_produce_equivalent_ir`（全 7
  非 Default kind）/ `native_and_mermaid_actor_produce_equivalent_ir`（Actor のみ、
  Mermaid scope 相当）: 成功
- layout の非 Default `«kind»` stereotype 行、Default participant geometry 不変、
  混在図（Default + Actor + Boundary）の header 高さ統一 tests: 成功
- 全 backend mapping（SVG / PNG / terminal は Scene Text 経由、draw.io /
  Excalidraw / PPTX は header ラベル反映、DOT は `UnsupportedDiagram` 維持）:
  成功。既存 all-Default golden の bytes は不変
- M3x0 exchange exporter contract の participant kind parity と future variant
  拒否（`validate_export_semantics`）: 成功
- `cargo fmt --check` / `cargo check --workspace` / `cargo test --workspace`
  （`UPDATE_GOLDEN` なし、全 workspace 緑）/ `cargo clippy --workspace
  --all-targets -- -D warnings` / `git diff --check`: 成功
- 既存 golden 差分0（`git diff --stat tests/golden` 空、新規は
  `seq_participant_kinds` / `mermaid_seq_actor` の untracked のみ）
- 独立レビュー（Opus）: blocking / major finding なし（verdict: ship）。
  minor 3件に対応 — DSL の keyword-as-id / formatter idempotency test 追加、
  提示系 fallback arm への意図コメント追加、V8 wire エラーメッセージに
  participant kind を追記。N2（`ParticipantKind` の stereotype ヘルパ共有化）は
  NodeKind と同一作法のため今回見送り

## M3b2 の検証状況

- schema V10 migration（V1-V9 の lossless upgrade）と V2-V9 document の新 arrow
  （Open / Cross / Circle head または非 None tail）明示拒否 test
  （`message_arrows_require_schema_v10`、全 25 head/tail 組合せ × 各 legacy
  fixture で `is_ok()==legacy_ok` を検証）: 成功。V1 は wire arm 先行拒否で除外
- **共有 `ArrowType` 不変**（Triangle / None のまま、Cross 等の混入なし）を確認。
  新 marker は `MessageArrow` に隔離
- default（head=Filled / tail=None）で layout 座標・全 backend 出力が既存と
  byte 一致（直線 / self-loop の retraction・triangle 頂点が式レベルで一致、
  drawio/excalidraw/pptx の default fragment も従来一致）→ 既存 seq golden 不変
- native DSL の `head` / `tail` modifier parse、非予約語（id 利用）、formatter
  idempotency・canonical 順・default 省略、graph/state 誤用と未知値の明示エラー
  tests: 成功
- Mermaid `-)` / `-x` / `<<->>` と `->`/`-->`→None 是正、PlantUML `->>`→Open /
  `->x` / `->o` / `<->` tests: 成功。既存 `*.mmd` golden が `->`/`-->` 未使用を
  grep 確認済で golden 差分なし
- 等価性: native↔PlantUML / native↔Mermaid の各 arrow 形が同一 IR になる CLI
  test: 成功
- 全 backend mapping と `MessageArrow` future variant の全 match arm 明示処理
  （layout / contract / drawio / excalidraw / pptx）: 成功。svg/png/term は
  lowered Scene を消費
- M3x0 contract の head / tail parity と未知 variant 拒否: 成功
- `cargo fmt --check` / `cargo check --workspace` / `cargo test --workspace`
  （全 workspace 緑、731 passed）/ `cargo clippy --workspace --all-targets --
  -D warnings` / `git diff --check`: 成功
- 既存 golden 差分0（`git diff --stat tests/golden` 空、新規は
  `seq_message_arrows` / `mermaid_seq_arrows` / `plantuml_seq_arrows` の
  untracked のみ）
- 独立レビュー（Opus）: blocking / major finding なし（verdict: ship）。
  実装者自己申告 6 点はすべて問題なし〜minor（drawio classic 一貫性・pptx
  diamond / excalidraw bar の Cross 損失近似は doc-comment 明示で silent でない・
  Circle overshoot は bounds 取込みで clip なし）。nit 2件のうち formatter の
  future-variant fallback に意図コメントを追加、PlantUML `->x` word-boundary は
  テスト済みトレードオフとして据え置き

## M3b3 の検証状況

- schema V11 migration と V2-V10 の Note item 明示拒否
  （`sequence_notes_require_schema_v11`、LeftOf / RightOf / Over 網羅）、V11
  round-trip、CURRENT=V11 に伴う numeric 境界 / upgrade tests 更新: 成功
- 全 9 個の `*_supported_in` gate に V11 が入っていることを確認（漏れなし）
- native DSL の `note over / left of / right of`、宣言順 interleave、graph / state
  での使用禁止、left/right の複数 target 拒否、unknown participant、formatter
  idempotency tests: 成功
- Mermaid `notes_parse_and_preserve_source_order` / `note_left_of_rejects_multiple_targets`、
  PlantUML `single_line_note_is_supported_and_ordered` /
  `multi_line_note_block_is_unsupported` / `hnote_is_unsupported`: 成功
- `native_mermaid_plantuml_notes_produce_equivalent_ir`（3 frontend が同一 note IR）:
  成功
- contract の item-parity 突合（length + variant 一致 zip）、note geometry 検証、
  `NotePosition` future variant 拒否と既存 sequence contract テスト更新: 成功
- `notes_map_across_all_backends`（drawio `shape=note`×3・excalidraw rectangle+text・
  pptx rect+text・SVG / terminal に note text）: 成功
- 新規 `seq_notes`（`.kzd` / `.svg` / `.txt` / `.png` / `.drawio` / `.excalidraw` /
  `.pptx`）・`mermaid_seq_notes`（`.mmd` / `.svg`）・`plantuml_seq_notes`
  （`.puml` / `.svg`）golden 追加。既存 golden bytes は変更なし
  （`git status` は新規 untracked のみ）
- `cargo fmt --all --check` / `cargo check --workspace` /
  `cargo clippy --workspace --all-targets -- -D warnings` /
  `cargo test --workspace`（`UPDATE_GOLDEN` なし、741 tests・33 binaries 緑） /
  `git diff --check`: すべて成功
- 独立レビュー（Opus）: blocking / major finding なし。IR gate 全数 V11 確認、
  item-parity 契約の fail-closed（variant 交差は明示 mismatch）、note 無し経路の
  col_x 式レベル不変（既存 golden byte 一致で裏付け）、bounds 正規化で note rect /
  text_anchor も平行移動、を確認。ロードマップ契約要件の frontend 等価テストが
  未実装だったため review 側で追加。既知の制限: Excalidraw note は矩形近似
  （doc-comment 明示）、PlantUML block note 未対応、note 塗り無しで lifeline 透過
  （M4 の paint primitive 送り）

## 再開時の確認事項

1. M3b1 / M3b2 / M3b3 は実装・検証・独立レビュー済み（M3b3 は未コミット、作業ツリー上）
2. 次は **M3b4（divider / delay / reference、schema V12）**。item 列一般化は
   M3b3 で完了しているので、新 item variant を `SequenceItem` /
   `SequenceItemLayout` に足す形で載る
3. M3b5 activation bar（V13、活性区間モデル新設） / M3b6 create / destroy（V14） /
   M3b7 fragment loop/alt/opt/par/critical/break（V15、items 平坦 Vec→再帰木、
   最も侵襲的で最後）
4. 実装後は別担当の独立レビューと root 総合レビューを行い、既存 golden 差分0を確認する
5. Sequence（M3b1-b7）-> State -> Class -> ER の順で M3 を完了する
