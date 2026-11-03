use crate::{
    application::media_url::{validate_media_url, ValidatedMediaUrl},
    domain::media::{AnalysisErrorCode, MediaInfo},
};

/// Native metadata producer port. Implementations must return only stable public errors.
pub trait MetadataAnalyzer: Send + Sync {
    fn analyze(&self, source: &ValidatedMediaUrl) -> Result<MediaInfo, AnalysisErrorCode>;
}

pub struct AnalyzeMediaService<'a> {
    analyzer: &'a dyn MetadataAnalyzer,
}

impl<'a> AnalyzeMediaService<'a> {
    pub fn new(analyzer: &'a dyn MetadataAnalyzer) -> Self {
        Self { analyzer }
    }

    pub fn analyze(&self, input: &str) -> Result<MediaInfo, AnalysisErrorCode> {
        let source = validate_media_url(input)?;
        self.analyze_validated(&source)
    }

    pub fn analyze_validated(
        &self,
        source: &ValidatedMediaUrl,
    ) -> Result<MediaInfo, AnalysisErrorCode> {
        let media = self.analyzer.analyze(source)?;
        media.validate()?;
        if media.platform != source.platform() || media.media_id != source.media_id() {
            return Err(AnalysisErrorCode::InvalidMetadata);
        }
        Ok(media)
    }
}
