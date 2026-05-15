//! API 响应格式
//!
//! 本模块定义了统一的 API 响应格式和辅助函数。

use axum::{
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

/// API 响应包装器
///
/// 用于统一成功响应的 JSON 格式。
///
/// # Type Parameters
/// * `T` - 实际的数据类型
#[derive(Debug, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    /// 是否成功
    pub success: bool,
    /// 响应数据
    pub data: T,
}

impl<T> ApiResponse<T> {
    /// 创建新的成功响应
    ///
    /// # Arguments
    /// * `data` - 响应数据
    ///
    /// # Returns
    /// * `Self` - 新创建的成功响应
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data,
        }
    }

    /// 将数据映射为新的类型
    ///
    /// # Type Parameters
    /// * `U` - 目标数据类型
    ///
    /// # Arguments
    /// * `f` - 转换函数
    ///
    /// # Returns
    /// * `ApiResponse<U>` - 转换后的响应
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> ApiResponse<U> {
        ApiResponse {
            success: self.success,
            data: f(self.data),
        }
    }
}

impl<T> IntoResponse for ApiResponse<T>
where
    T: Serialize,
{
    fn into_response(self) -> Response {
        Json(self).into_response()
    }
}

/// 空响应数据
///
/// 用于不需要返回数据的成功响应。
#[derive(Debug, Serialize, Deserialize)]
pub struct EmptyResponse {}

impl EmptyResponse {
    /// 创建空响应
    ///
    /// # Returns
    /// * `Self` - 空响应实例
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for EmptyResponse {
    fn default() -> Self {
        Self::new()
    }
}

/// 创建成功响应
///
/// # Type Parameters
/// * `T` - 数据类型
///
/// # Arguments
/// * `data` - 响应数据
///
/// # Returns
/// * `ApiResponse<T>` - 成功响应包装器
pub fn success<T>(data: T) -> ApiResponse<T> {
    ApiResponse::success(data)
}

/// 创建空成功响应
///
/// # Returns
/// * `ApiResponse<EmptyResponse>` - 空成功响应
pub fn empty_success() -> ApiResponse<EmptyResponse> {
    ApiResponse::success(EmptyResponse::new())
}

/// 分页信息
///
/// 用于分页响应的元数据。
#[derive(Debug, Serialize, Deserialize)]
pub struct Pagination {
    /// 当前页码（从 1 开始）
    pub page: u32,
    /// 每页数量
    pub page_size: u32,
    /// 总记录数
    pub total: u32,
    /// 总页数
    pub total_pages: u32,
}

impl Pagination {
    /// 创建新的分页信息
    ///
    /// # Arguments
    /// * `page` - 当前页码
    /// * `page_size` - 每页数量
    /// * `total` - 总记录数
    ///
    /// # Returns
    /// * `Self` - 新创建的分页信息
    pub fn new(page: u32, page_size: u32, total: u32) -> Self {
        let total_pages = if total == 0 {
            0
        } else {
            total.div_ceil(page_size)
        };

        Self {
            page,
            page_size,
            total,
            total_pages,
        }
    }
}

/// 带分页的响应数据
///
/// # Type Parameters
/// * `T` - 数据类型（通常是 Vec）
#[derive(Debug, Serialize, Deserialize)]
pub struct PagedResponse<T> {
    /// 分页数据
    pub data: T,
    /// 分页元数据
    pub pagination: Pagination,
}

impl<T> PagedResponse<T> {
    /// 创建新的分页响应
    ///
    /// # Arguments
    /// * `data` - 分页数据
    /// * `page` - 当前页码
    /// * `page_size` - 每页数量
    /// * `total` - 总记录数
    ///
    /// # Returns
    /// * `Self` - 新创建的分页响应
    pub fn new(data: T, page: u32, page_size: u32, total: u32) -> Self {
        Self {
            data,
            pagination: Pagination::new(page, page_size, total),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_response_success() {
        let response = ApiResponse::success("test data");
        assert!(response.success);
        assert_eq!(response.data, "test data");
    }

    #[test]
    fn test_api_response_map() {
        let response = ApiResponse::success(5);
        let mapped = response.map(|x| x * 2);
        assert!(mapped.success);
        assert_eq!(mapped.data, 10);
    }

    #[test]
    fn test_empty_response() {
        let response = EmptyResponse::new();
        let json = serde_json::to_string(&response).unwrap();
        assert_eq!(json, "{}");
    }

    #[test]
    fn test_pagination_new() {
        let pagination = Pagination::new(1, 10, 25);
        assert_eq!(pagination.page, 1);
        assert_eq!(pagination.page_size, 10);
        assert_eq!(pagination.total, 25);
        assert_eq!(pagination.total_pages, 3);
    }

    #[test]
    fn test_pagination_zero_total() {
        let pagination = Pagination::new(1, 10, 0);
        assert_eq!(pagination.total_pages, 0);
    }

    #[test]
    fn test_paged_response() {
        let data = vec![1, 2, 3];
        let response = PagedResponse::new(data.clone(), 1, 10, 3);
        assert_eq!(response.data, data);
        assert_eq!(response.pagination.total, 3);
    }
}
