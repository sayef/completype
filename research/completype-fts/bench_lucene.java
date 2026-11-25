import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import org.apache.lucene.analysis.core.WhitespaceAnalyzer;
import org.apache.lucene.document.Document;
import org.apache.lucene.document.Field;
import org.apache.lucene.document.StoredField;
import org.apache.lucene.document.TextField;
import org.apache.lucene.index.DirectoryReader;
import org.apache.lucene.index.IndexWriter;
import org.apache.lucene.index.IndexWriterConfig;
import org.apache.lucene.index.Term;
import org.apache.lucene.search.BooleanClause;
import org.apache.lucene.search.BooleanQuery;
import org.apache.lucene.search.IndexSearcher;
import org.apache.lucene.search.TermQuery;
import org.apache.lucene.search.similarities.BM25Similarity;
import org.apache.lucene.store.FSDirectory;

class BenchLucene {
    public static void main(String[] args) throws Exception {
        var input = new BufferedReader(new InputStreamReader(System.in, StandardCharsets.UTF_8));
        try (var directory = FSDirectory.open(Path.of(args[0]));
             var analyzer = new WhitespaceAnalyzer()) {
            var config = new IndexWriterConfig(analyzer).setRAMBufferSizeMB(128);
            try (var writer = new IndexWriter(directory, config)) {
                String row;
                while ((row = input.readLine()) != null && !row.isEmpty()) {
                    int tab = row.indexOf('\t');
                    var doc = new Document();
                    doc.add(new StoredField("id", row.substring(0, tab)));
                    doc.add(new TextField("body", row.substring(tab + 1), Field.Store.NO));
                    writer.addDocument(doc);
                }
            }
            try (var reader = DirectoryReader.open(directory)) {
                var searcher = new IndexSearcher(reader);
                searcher.setSimilarity(new BM25Similarity(1.2f, 0.75f));
                var stored = searcher.storedFields();
                System.out.println("READY");
                String row;
                while ((row = input.readLine()) != null) {
                    var builder = new BooleanQuery.Builder();
                    for (String term : row.split(" ")) {
                        if (!term.isEmpty()) {
                            builder.add(new TermQuery(new Term("body", term)), BooleanClause.Occur.SHOULD);
                        }
                    }
                    var ids = new StringBuilder();
                    for (var hit : searcher.search(builder.build(), 100).scoreDocs) {
                        if (!ids.isEmpty()) ids.append('\t');
                        ids.append(stored.document(hit.doc).get("id"));
                    }
                    System.out.println(ids);
                }
            }
        }
    }
}
