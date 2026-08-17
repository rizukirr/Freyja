//! Putting an image in a prompt.
//!
//! An image is one more part of a user turn, alongside text. Freyja carries it
//! by reference or inline, and each dialect rearranges it into whatever shape
//! that vendor expects.
//!
//! ```sh
//! cargo run --example images                          # the embedded image
//! IMAGE_URL=https://example.com/cat.png cargo run --example images
//! ```

use freyja::{Client, EndpointPreset, Error, GenerateRequest, InputContent, Message, Role};

/// A 64×64 PNG: a red circle on white, embedded so the example needs no network
/// beyond the model call and no third-party URL that might rot.
///
/// A `data:` URI costs about a third more bytes than the file, since base64
/// encodes three bytes into four. That is charged to you as input tokens, so
/// prefer a URL for anything large or reused.
const RED_CIRCLE: &str = concat!(
    "data:image/png;base64,",
    "iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAr0lEQVR42u3auxGAMAwE0euIGfov",
    "hUqISIk8/oAln9dDAfsyI1n34kcAAAAAAAAAgPe5zqPw5QWUu/+TaH76twxFpX/FUHj9oEHh6YMM",
    "5anvMyhVfYdhJ8Cc+laDEtY3GfYAzK+vN2wAiKqvNABIDoitrzEAAAAAAAAAzgDuQgD4obEAMJVI",
    "AFh+sMVsNAdg+fG6w4LDYcVksuRzWLOaLLpNnhrwWgUAAAAAAADoOA/qo8ml7jf7HgAAAABJRU5E",
    "rkJggg=="
);

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let provider = EndpointPreset::OpenAi;
    let Some(client) = Client::from_env(provider) else {
        eprintln!("{} is missing or empty", provider.api_key_env());
        return;
    };

    // A hosted URL works the same way, and is the better choice when the image
    // is large or sent more than once.
    let image = std::env::var("IMAGE_URL").unwrap_or_else(|_| RED_CIRCLE.to_string());
    if image.starts_with("http") {
        println!("using IMAGE_URL: {image}\n");
    }

    // One turn, two parts. Order matters: put the text after the image and you
    // are asking about something the model has not seen yet, which reads to it
    // exactly as it reads here.
    let request = GenerateRequest::new().message(Message::new(
        Role::User,
        [
            InputContent::ImageUrl(image),
            InputContent::Text("What shape and colour is this? Answer in five words.".into()),
        ],
    ));

    match client.generate(&request).await {
        Ok(response) => {
            println!("{}", response.output_text());
            if let Some(usage) = response.usage {
                // Images are billed as input tokens, and a large one costs more
                // than the sentence next to it by a wide margin.
                println!("\n{} input tokens", usage.input_tokens);
            }
        }
        Err(error) => eprintln!("{} failed: {error}", error.endpoint()),
    }

    demonstrate_placement(&client).await;
}

/// Where an image may not go.
///
/// Images belong on user turns, and the refusal below names the nearer of two
/// reasons. On most dialects a system turn is a text-only field, so it reports
/// `non-text content in system/developer messages`. On OpenAI Chat Completions
/// a system turn is an ordinary message with nothing to refuse on those
/// grounds, so the same call reports `images outside user messages` instead —
/// which is also what an image on an assistant turn reports everywhere, an
/// assistant turn recording what the model said rather than what it was shown.
///
/// Either way the refusal is local. Freyja will not drop the image and leave
/// you wondering why the answer ignored it.
async fn demonstrate_placement(client: &Client) {
    let request = GenerateRequest::new()
        .message(Message::new(
            Role::System,
            [InputContent::ImageUrl(RED_CIRCLE.into())],
        ))
        .message(Message::text(Role::User, "Describe the image above."));

    println!("\n== the same image, on a system turn ==");
    match client.generate(&request).await {
        Ok(response) => println!("{}", response.output_text()),
        Err(Error::UnsupportedCapability { capability, .. }) => {
            println!("refused: cannot express {capability} (no request was sent)");
        }
        Err(error) => eprintln!("{} failed: {error}", error.endpoint()),
    }
}
