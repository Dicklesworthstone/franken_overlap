use franken_overlap::{IndexBuilder, IndexConfig, SearchOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = IndexBuilder::new(IndexConfig::default())?;
    builder.add_document(
        "source.txt",
        "The team checked every instrument and published the raw observations before proposing an explanation.",
    )?;
    let index = builder.build()?;
    for hit in index.search(
        "They checked all the instruments and published the raw observations before suggesting an explanation.",
        &SearchOptions {
            minimum_similarity: 0.2,
            ..SearchOptions::default()
        },
    )? {
        println!("{} {:.3}: {}", hit.path, hit.combined_score, hit.matched_text);
    }
    Ok(())
}
