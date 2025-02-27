package com.arnisplugin;

import org.bukkit.plugin.java.JavaPlugin;
import javax.mail.*;
import javax.mail.internet.*;
import java.util.Properties;

public class ArnisPlugin extends JavaPlugin {

    @Override
    public void onEnable() {
        getLogger().info("Arnis Plugin aktiviert!");
        sendEmail("Dies ist ein Test", "Dies ist der Inhalt der E-Mail.", "timgigerich2@gmail.com");
    }

    public void sendEmail(String subject, String body, String toEmail) {
        final String username = "deineEmail@gmail.com"; // Absender E-Mail (z.B. Gmail)
        final String password = "deinAppPassword"; // E-Mail Passwort (besser: App-Passwort, nicht das Haupt-Passwort)

        Properties properties = new Properties();
        properties.put("mail.smtp.auth", "true");
        properties.put("mail.smtp.starttls.enable", "true");
        properties.put("mail.smtp.host", "smtp.gmail.com");
        properties.put("mail.smtp.port", "587");

        Session session = Session.getInstance(properties, new Authenticator() {
            @Override
            protected PasswordAuthentication getPasswordAuthentication() {
                return new PasswordAuthentication(username, password);
            }
        });

        try {
            Message message = new MimeMessage(session);
            message.setFrom(new InternetAddress(username));
            message.setRecipients(Message.RecipientType.TO, InternetAddress.parse(toEmail));
            message.setSubject(subject);
            message.setText(body);

            Transport.send(message);
            getLogger().info("E-Mail erfolgreich gesendet!");

        } catch (MessagingException e) {
            throw new RuntimeException(e);
        }
    }
}
