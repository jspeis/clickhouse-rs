use std::cmp;
use std::fmt;

use chrono_tz::Tz;

use binary::{protocol, Encoder, ReadEx};
use block::chunk_iterator::ChunkIterator;
use block::BlockInfo;
use column::{self, Column, ColumnFrom};
use types::{FromSql, FromSqlError, FromSqlResult};
use ClickhouseResult;

const INSERT_BLOCK_SIZE: usize = 1048576;

pub trait ColumnIdx {
    fn get_index(&self, columns: &Vec<Column>) -> FromSqlResult<usize>;
}

#[derive(Default)]
pub struct Block {
    info: BlockInfo,
    columns: Vec<Column>,
}

pub trait BlockEx {
    fn write(&self, encoder: &mut Encoder);
    fn send_data(&self, encoder: &mut Encoder);
    fn concat(blocks: &[Block]) -> Block;
    fn chunks(&self, n: usize) -> ChunkIterator;
}

impl PartialEq<Block> for Block {
    fn eq(&self, other: &Block) -> bool {
        if self.columns.len() != other.columns.len() {
            return false;
        }

        for i in 0..self.columns.len() {
            if self.columns[i] != other.columns[i] {
                return false;
            }
        }

        return true;
    }
}

impl Clone for Block {
    fn clone(&self) -> Self {
        Block {
            info: self.info.clone(),
            columns: self.columns.iter().map(|c| (*c).clone()).collect(),
        }
    }
}

impl AsRef<Block> for Block {
    fn as_ref(&self) -> &Block {
        self
    }
}

impl ColumnIdx for usize {
    fn get_index(&self, _: &Vec<Column>) -> FromSqlResult<usize> {
        Ok(*self)
    }
}

impl<'a> ColumnIdx for &'a str {
    fn get_index(&self, columns: &Vec<Column>) -> FromSqlResult<usize> {
        match columns
            .iter()
            .enumerate()
            .find(|(_, column)| column.name() == *self)
        {
            None => Err(FromSqlError::OutOfRange),
            Some((index, _)) => Ok(index),
        }
    }
}

impl Block {
    /// Constructs a new, empty Block.
    pub fn new() -> Block {
        Block::default()
    }

    pub fn load<R: ReadEx>(reader: &mut R, tz: Tz) -> ClickhouseResult<Block> {
        let mut block = Block::default();

        block.info = BlockInfo::read(reader)?;

        let num_columns = reader.read_uvarint()?;
        let num_rows = reader.read_uvarint()?;

        for _ in 0..num_columns {
            let column = Column::read(reader, num_rows as usize, tz)?;
            block.append_column(column);
        }

        Ok(block)
    }

    /// Return the number of rows in the current block.
    pub fn row_count(&self) -> usize {
        match self.columns.first() {
            None => 0,
            Some(column) => column.len(),
        }
    }

    /// Return the number of columns in the current block.
    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    pub fn columns(&self) -> &Vec<Column> {
        &self.columns
    }

    fn append_column(&mut self, column: Column) {
        let column_len = column.len();

        if !self.columns.is_empty() && self.row_count() != column_len {
            panic!("all columns in block must have same count of rows.")
        }

        self.columns.push(column);
    }

    /// Get the value of a particular cell of the block.
    pub fn get<'a, T, I>(&'a self, row: usize, col: I) -> FromSqlResult<T>
    where
        T: FromSql<'a>,
        I: ColumnIdx,
    {
        let column_index = col.get_index(self.columns())?;
        T::from_sql(self.columns[column_index].at(row))
    }

    /// Add new column into this block
    pub fn add_column<S>(mut self, name: &str, values: S) -> Block
    where
        S: ColumnFrom,
    {
        let data = S::column_from(values);
        let column = column::new_column(name, data);

        self.append_column(column);
        self
    }

    /// Returns true if the block contains no elements.
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }
}

impl BlockEx for Block {
    fn write(&self, encoder: &mut Encoder) {
        self.info.write(encoder);
        encoder.uvarint(self.column_count() as u64);
        encoder.uvarint(self.row_count() as u64);

        for column in &self.columns {
            column.write(encoder);
        }
    }

    fn send_data(&self, encoder: &mut Encoder) {
        encoder.uvarint(protocol::CLIENT_DATA);
        encoder.string(""); // temporary table
        for chunk in self.chunks(INSERT_BLOCK_SIZE) {
            chunk.write(encoder);
        }
    }

    fn concat(blocks: &[Block]) -> Block {
        let first = blocks.first().expect("blocks should not be empty.");

        for block in blocks {
            assert_eq!(
                first.column_count(),
                block.column_count(),
                "all block should have the same columns."
            );
        }

        let num_columns = first.column_count();
        let mut columns = Vec::with_capacity(num_columns);
        for i in 0_usize..num_columns {
            let chunks = blocks.iter().map(|block| &block.columns[i]);
            columns.push(Column::concat(chunks));
        }

        Block {
            info: first.info,
            columns,
        }
    }

    fn chunks(&self, n: usize) -> ChunkIterator {
        ChunkIterator::new(n, self)
    }
}

impl fmt::Debug for Block {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        let titles: Vec<&str> = self.columns.iter().map(|column| column.name()).collect();

        let cells: Vec<_> = self.columns.iter().map(|col| text_cells(&col)).collect();

        let titles_len = titles
            .iter()
            .map(|t| t.chars().count())
            .zip(cells.iter().map(column_width))
            .map(|(a, b)| cmp::max(a, b))
            .collect();

        print_line(f, &titles_len, "\n┌", '┬', "┐\n")?;

        for (i, title) in titles.iter().enumerate() {
            write!(f, "│{:>width$} ", title, width = titles_len[i] + 1)?;
        }
        write!(f, "│")?;

        if self.row_count() > 0 {
            print_line(f, &titles_len, "\n├", '┼', "┤\n")?;
        }

        for j in 0..self.row_count() {
            for (i, col) in cells.iter().enumerate() {
                write!(f, "│{:>width$} ", col[j], width = titles_len[i] + 1)?;
            }

            let new_line = (j + 1) != self.row_count();
            write!(f, "│{}", if new_line { "\n" } else { "" })?;
        }

        return print_line(f, &titles_len, "\n└", '┴', "┘");
    }
}

fn column_width(column: &Vec<String>) -> usize {
    column.iter().map(|cell| cell.len()).max().unwrap_or(0)
}

fn print_line(
    f: &mut fmt::Formatter,
    lens: &Vec<usize>,
    left: &str,
    center: char,
    right: &str,
) -> Result<(), fmt::Error> {
    write!(f, "{}", left)?;
    for (i, len) in lens.iter().enumerate() {
        if i != 0 {
            write!(f, "{}", center)?;
        }

        write!(f, "{:─>width$}", "", width = len + 2)?;
    }
    write!(f, "{}", right)
}

fn text_cells(data: &Column) -> Vec<String> {
    (0..data.len()).map(|i| format!("{}", data.at(i))).collect()
}

#[cfg(test)]
mod test {
    use std::io::Cursor;

    use chrono_tz::Tz;

    use binary::Encoder;
    use block::{Block, BlockEx};

    #[test]
    fn test_write_default() {
        let expected = [1, 0, 2, 255, 255, 255, 255, 0, 0, 0];
        let mut encoder = Encoder::new();
        Block::default().write(&mut encoder);
        assert_eq!(encoder.get_buffer_ref(), &expected)
    }

    #[test]
    fn test_read_empty_block() {
        let source = [1, 0, 2, 255, 255, 255, 255, 0, 0, 0];
        let mut cursor = Cursor::new(&source[..]);
        match Block::load(&mut cursor, Tz::Zulu) {
            Ok(block) => assert!(block.is_empty()),
            Err(_) => panic!("test_read_empty_block"),
        }
    }

    #[test]
    fn test_empty() {
        assert!(Block::default().is_empty())
    }

    #[test]
    fn test_column_and_rows() {
        let block = Block::new()
            .add_column("hello_id", vec![5_u32, 6_u32])
            .add_column("value", vec!["lol", "zuz"]);

        assert_eq!(block.column_count(), 2);
        assert_eq!(block.row_count(), 2);
    }

    #[test]
    fn test_concat() {
        let block_a = make_block();
        let block_b = make_block();

        let actual = Block::concat(&vec![block_a, block_b]);
        assert_eq!(actual.row_count(), 4);
        assert_eq!(actual.column_count(), 1);

        assert_eq!(
            "5446d186-4e90-4dd8-8ec1-f9a436834613".to_string(),
            actual.get::<String, _>(0, 0).unwrap()
        );
        assert_eq!(
            "f7cf31f4-7f37-4e27-91c0-5ac0ad0b145b".to_string(),
            actual.get::<String, _>(1, 0).unwrap()
        );
        assert_eq!(
            "5446d186-4e90-4dd8-8ec1-f9a436834613".to_string(),
            actual.get::<String, _>(2, 0).unwrap()
        );
        assert_eq!(
            "f7cf31f4-7f37-4e27-91c0-5ac0ad0b145b".to_string(),
            actual.get::<String, _>(3, 0).unwrap()
        );
    }

    fn make_block() -> Block {
        Block::new().add_column(
            "9b96ad8b-488a-4fef-8087-8a9ae4800f00",
            vec![
                "5446d186-4e90-4dd8-8ec1-f9a436834613".to_string(),
                "f7cf31f4-7f37-4e27-91c0-5ac0ad0b145b".to_string(),
            ],
        )
    }

    #[test]
    fn test_chunks() {
        let first = Block::new().add_column("A", vec![1, 2]);
        let second = Block::new().add_column("A", vec![3, 4]);
        let third = Block::new().add_column("A", vec![5]);

        let block = Block::new().add_column("A", vec![1, 2, 3, 4, 5]);
        let mut iter = block.chunks(2);

        assert_eq!(Some(first), iter.next());
        assert_eq!(Some(second), iter.next());
        assert_eq!(Some(third), iter.next());
        assert_eq!(None, iter.next());
    }

    #[test]
    fn test_chunks_of_empty_block() {
        let block = Block::default();
        assert_eq!(1, block.chunks(100500).count());
        assert_eq!(Some(block.clone()), block.chunks(100500).next());
    }
}