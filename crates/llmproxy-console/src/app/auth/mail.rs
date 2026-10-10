use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::authentication::Credentials,
};
use llmproxy_store::auth::{EmailCode, EmailPurpose};
use llmproxy_store::settings::MailDeliverySettings;
use std::{io, net::IpAddr, time::Duration};

pub(crate) struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl Mailer {
    pub(crate) fn new(config: MailDeliverySettings) -> io::Result<Self> {
        let MailDeliverySettings { settings, password } = config;
        let host = &settings.host;
        let failure =
            || io::Error::other("SMTP 配置无效，请检查服务器、端口、TLS、发件人及认证配置");
        let mut builder = match settings.tls.as_str() {
            "starttls" => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host).map_err(|_| failure())?
            }
            "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(host).map_err(|_| failure())?,
            "none"
                if host == "localhost"
                    || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()) =>
            {
                AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
            }
            _ => return Err(failure()),
        }
        .port(settings.port)
        .timeout(Some(Duration::from_secs(10)));
        if !settings.username.is_empty() {
            builder = builder.credentials(Credentials::new(settings.username, password));
        }
        let from = settings.from.parse().map_err(|_| failure())?;
        Ok(Self {
            transport: builder.build(),
            from: Mailbox::new(Some("LLMProxy".into()), from),
        })
    }

    pub(super) async fn send(&self, code: EmailCode) -> Result<(), String> {
        let action = match code.purpose {
            EmailPurpose::Register => "注册邮箱验证",
            EmailPurpose::ResetPassword => "重置密码",
        };
        let message = Message::builder().header(ContentType::TEXT_PLAIN).from(self.from.clone())
            .to(code.email.parse().map_err(|_| "邮箱地址无效".to_owned())?)
            .subject(format!("LLMProxy {action}"))
            .body(format!("你的{action}验证码是：{}\n\n验证码 10 分钟内有效，仅可使用一次。\n如果这不是你发起的操作，请忽略此邮件。", code.code))
            .map_err(|_| "无法生成验证邮件".to_owned())?;
        self.deliver(message).await
    }

    pub(crate) async fn send_test(&self, recipient: &str) -> Result<(), String> {
        let message = Message::builder()
            .header(ContentType::TEXT_PLAIN)
            .from(self.from.clone())
            .to(recipient
                .parse()
                .map_err(|_| "请输入有效的收件邮箱".to_owned())?)
            .subject("LLMProxy 邮件服务测试")
            .body("这是一封 LLMProxy 测试邮件。收到此邮件表示当前邮件服务可以正常发送。".to_owned())
            .map_err(|_| "无法生成测试邮件".to_owned())?;
        self.deliver(message).await
    }

    async fn deliver(&self, message: Message) -> Result<(), String> {
        self.transport
            .send(message)
            .await
            .map(|_| ())
            .map_err(|_| "邮件发送失败，请检查邮件服务器、端口、加密方式及认证信息".to_owned())
    }
}
