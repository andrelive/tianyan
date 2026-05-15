use serde::{Deserialize, Serialize};

/// 向量嵌入表示。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Embedding {
    /// 向量数据
    pub vector: Vec<f32>,
    /// 向量维度
    pub dimension: usize,
}

impl Embedding {
    /// 创建新的嵌入向量。
    pub fn new(vector: Vec<f32>) -> Self {
        let dimension = vector.len();
        Self { vector, dimension }
    }

    /// 创建指定维度的空嵌入向量。
    pub fn zero(dimension: usize) -> Self {
        Self {
            vector: vec![0.0; dimension],
            dimension,
        }
    }

    /// 计算与另一个嵌入向量的余弦相似度。
    pub fn cosine_similarity(&self, other: &Embedding) -> f32 {
        if self.dimension != other.dimension {
            return 0.0;
        }

        let dot_product: f32 = self
            .vector
            .iter()
            .zip(other.vector.iter())
            .map(|(a, b)| a * b)
            .sum();

        let norm_a: f32 = self.vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = other.vector.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }

        dot_product / (norm_a * norm_b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedding_cosine_similarity() {
        let a = Embedding::new(vec![1.0, 0.0, 0.0]);
        let b = Embedding::new(vec![1.0, 0.0, 0.0]);
        assert!((a.cosine_similarity(&b) - 1.0).abs() < 0.001);

        let c = Embedding::new(vec![0.0, 1.0, 0.0]);
        assert!((a.cosine_similarity(&c) - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_embedding_zero() {
        let embedding = Embedding::zero(768);
        assert_eq!(embedding.dimension, 768);
        assert_eq!(embedding.vector.len(), 768);
        assert!(embedding.vector.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn test_embedding_different_dimensions() {
        let a = Embedding::new(vec![1.0, 0.0, 0.0]);
        let b = Embedding::new(vec![1.0, 0.0, 0.0, 0.0]);
        assert!((a.cosine_similarity(&b) - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_embedding_zero_vector_similarity() {
        let a = Embedding::zero(3);
        let b = Embedding::new(vec![1.0, 0.0, 0.0]);
        assert!((a.cosine_similarity(&b) - 0.0).abs() < 0.001);
    }
}
