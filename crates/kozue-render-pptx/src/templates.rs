//! Constant OOXML template parts for the minimal `.pptx` package.
//!
//! Every part here is a **fixed string** — no per-diagram data flows into it.
//! Only `ppt/slides/slide1.xml` (built in `lib.rs`) is generated dynamically.
//!
//! These templates follow the well-known minimal-but-valid OOXML presentation
//! skeleton (the same shape python-pptx's default template and the
//! officeopenxml.com reference examples use): a single slide master, a single
//! blank slide layout, and a theme whose `<a:fmtScheme>` declares exactly the
//! 3 fill / 3 line / 3 effect / 3 background-fill styles PowerPoint requires
//! (fewer than 3 triggers a "needs repair" prompt on open).

pub const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

pub const CONTENT_TYPES: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
    "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">",
    "<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>",
    "<Default Extension=\"xml\" ContentType=\"application/xml\"/>",
    "<Override PartName=\"/ppt/presentation.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml\"/>",
    "<Override PartName=\"/ppt/slideMasters/slideMaster1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml\"/>",
    "<Override PartName=\"/ppt/slideLayouts/slideLayout1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml\"/>",
    "<Override PartName=\"/ppt/slides/slide1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>",
    "<Override PartName=\"/ppt/theme/theme1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.theme+xml\"/>",
    "<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>",
    "<Override PartName=\"/docProps/app.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.extended-properties+xml\"/>",
    "</Types>",
);

pub const ROOT_RELS: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
    "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"ppt/presentation.xml\"/>",
    "<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/>",
    "<Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties\" Target=\"docProps/app.xml\"/>",
    "</Relationships>",
);

pub const DOC_PROPS_APP: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
    "<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\" xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\">",
    "<Application>kozue</Application>",
    "<PresentationFormat>Widescreen</PresentationFormat>",
    "<Slides>1</Slides>",
    "<TitlesOfParts><vt:vector size=\"1\" baseType=\"lpstr\"><vt:lpstr>Slide 1</vt:lpstr></vt:vector></TitlesOfParts>",
    "<Company></Company>",
    "</Properties>",
);

/// `dcterms:created` / `dcterms:modified` are fixed per the crate's determinism
/// contract — never the wall-clock time.
pub const DOC_PROPS_CORE: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
    "<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" xmlns:dcmitype=\"http://purl.org/dc/dcmitype/\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">",
    "<dc:title>kozue diagram</dc:title>",
    "<dc:creator>kozue</dc:creator>",
    "<cp:lastModifiedBy>kozue</cp:lastModifiedBy>",
    "<dcterms:created xsi:type=\"dcterms:W3CDTF\">2024-01-01T00:00:00Z</dcterms:created>",
    "<dcterms:modified xsi:type=\"dcterms:W3CDTF\">2024-01-01T00:00:00Z</dcterms:modified>",
    "</cp:coreProperties>",
);

/// 16:9 widescreen slide size (12192000 x 6858000 EMU = 13.333in x 7.5in).
pub const PRESENTATION: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
    "<p:presentation xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\">",
    "<p:sldMasterIdLst><p:sldMasterId id=\"2147483648\" r:id=\"rId1\"/></p:sldMasterIdLst>",
    "<p:sldIdLst><p:sldId id=\"256\" r:id=\"rId2\"/></p:sldIdLst>",
    "<p:sldSz cx=\"12192000\" cy=\"6858000\" type=\"screen16x9\"/>",
    "<p:notesSz cx=\"6858000\" cy=\"9144000\"/>",
    "</p:presentation>",
);

pub const PRESENTATION_RELS: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
    "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster\" Target=\"slideMasters/slideMaster1.xml\"/>",
    "<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide1.xml\"/>",
    "<Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme\" Target=\"theme/theme1.xml\"/>",
    "</Relationships>",
);

pub const SLIDE_MASTER: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
    "<p:sldMaster xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\">",
    "<p:cSld>",
    "<p:bg><p:bgRef idx=\"1001\"><a:schemeClr val=\"bg1\"/></p:bgRef></p:bg>",
    "<p:spTree>",
    "<p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>",
    "<p:grpSpPr/>",
    "</p:spTree>",
    "</p:cSld>",
    "<p:clrMap bg1=\"lt1\" tx1=\"dk1\" bg2=\"lt2\" tx2=\"dk2\" accent1=\"accent1\" accent2=\"accent2\" accent3=\"accent3\" accent4=\"accent4\" accent5=\"accent5\" accent6=\"accent6\" hlink=\"hlink\" folHlink=\"folHlink\"/>",
    "<p:sldLayoutIdLst><p:sldLayoutId id=\"2147483649\" r:id=\"rId1\"/></p:sldLayoutIdLst>",
    "<p:txStyles>",
    "<p:titleStyle><a:lvl1pPr algn=\"ctr\"><a:defRPr sz=\"4400\"/></a:lvl1pPr></p:titleStyle>",
    "<p:bodyStyle><a:lvl1pPr><a:defRPr sz=\"3200\"/></a:lvl1pPr></p:bodyStyle>",
    "<p:otherStyle><a:lvl1pPr><a:defRPr sz=\"1800\"/></a:lvl1pPr></p:otherStyle>",
    "</p:txStyles>",
    "</p:sldMaster>",
);

pub const SLIDE_MASTER_RELS: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
    "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout\" Target=\"../slideLayouts/slideLayout1.xml\"/>",
    "<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme\" Target=\"../theme/theme1.xml\"/>",
    "</Relationships>",
);

pub const SLIDE_LAYOUT: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
    "<p:sldLayout xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" type=\"blank\" preserve=\"1\">",
    "<p:cSld name=\"Blank\">",
    "<p:spTree>",
    "<p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>",
    "<p:grpSpPr/>",
    "</p:spTree>",
    "</p:cSld>",
    "<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>",
    "</p:sldLayout>",
);

pub const SLIDE_LAYOUT_RELS: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
    "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster\" Target=\"../slideMasters/slideMaster1.xml\"/>",
    "</Relationships>",
);

/// Minimal valid Office theme. `<a:fmtScheme>` declares exactly 3 entries in
/// each of fillStyleLst / lnStyleLst / effectStyleLst / bgFillStyleLst, which
/// is the count PowerPoint's schema validator expects (fewer triggers "needs
/// repair" on open).
pub const THEME: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
    "<a:theme xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" name=\"Office Theme\">",
    "<a:themeElements>",
    "<a:clrScheme name=\"Office\">",
    "<a:dk1><a:sysClr val=\"windowText\" lastClr=\"000000\"/></a:dk1>",
    "<a:lt1><a:sysClr val=\"window\" lastClr=\"FFFFFF\"/></a:lt1>",
    "<a:dk2><a:srgbClr val=\"1F497D\"/></a:dk2>",
    "<a:lt2><a:srgbClr val=\"EEECE1\"/></a:lt2>",
    "<a:accent1><a:srgbClr val=\"4F81BD\"/></a:accent1>",
    "<a:accent2><a:srgbClr val=\"C0504D\"/></a:accent2>",
    "<a:accent3><a:srgbClr val=\"9BBB59\"/></a:accent3>",
    "<a:accent4><a:srgbClr val=\"8064A2\"/></a:accent4>",
    "<a:accent5><a:srgbClr val=\"4BACC6\"/></a:accent5>",
    "<a:accent6><a:srgbClr val=\"F79646\"/></a:accent6>",
    "<a:hlink><a:srgbClr val=\"0000FF\"/></a:hlink>",
    "<a:folHlink><a:srgbClr val=\"800080\"/></a:folHlink>",
    "</a:clrScheme>",
    "<a:fontScheme name=\"Office\">",
    "<a:majorFont><a:latin typeface=\"Calibri\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:majorFont>",
    "<a:minorFont><a:latin typeface=\"Calibri\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:minorFont>",
    "</a:fontScheme>",
    "<a:fmtScheme name=\"Office\">",
    "<a:fillStyleLst>",
    "<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>",
    "<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>",
    "<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>",
    "</a:fillStyleLst>",
    "<a:lnStyleLst>",
    "<a:ln w=\"9525\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln>",
    "<a:ln w=\"25400\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln>",
    "<a:ln w=\"38100\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln>",
    "</a:lnStyleLst>",
    "<a:effectStyleLst>",
    "<a:effectStyle><a:effectLst/></a:effectStyle>",
    "<a:effectStyle><a:effectLst/></a:effectStyle>",
    "<a:effectStyle><a:effectLst/></a:effectStyle>",
    "</a:effectStyleLst>",
    "<a:bgFillStyleLst>",
    "<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>",
    "<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>",
    "<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>",
    "</a:bgFillStyleLst>",
    "</a:fmtScheme>",
    "</a:themeElements>",
    "</a:theme>",
);

pub const SLIDE_RELS: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
    "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout\" Target=\"../slideLayouts/slideLayout1.xml\"/>",
    "</Relationships>",
);
