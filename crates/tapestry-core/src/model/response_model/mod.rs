//! Response model interface and implementations.

mod mock_response_model;
mod openai_response_model;
mod response_model_trait;

pub use mock_response_model::MockResponseModel;
pub use openai_response_model::OpenAiResponseModel;
pub use response_model_trait::ResponseModel;
