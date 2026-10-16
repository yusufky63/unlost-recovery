# SSS

[English](faq.md)

## Önemli bir şeyi az önce sildim. Şimdi ne yapmalıyım?

Bulunduğu sürücüyü kullanmayı bırakın. Oraya hiçbir şey kurmayın (kurtarma programları dahil), dosya indirmeyin ya da kaydetmeyin, arka planda yazan programları kapatın. Ardından şu sırayla bakın: Geri Dönüşüm Kutusu, Önceki sürümler (gölge kopyalar), bulut çöp kutuları (OneDrive, Google Drive, Dropbox), Dosya Geçmişi ve uygulamanın kendi otomatik kayıtları. Silinmiş dosya taramasına ancak bundan sonra geçin. Unlost'un ilk ekranı da aynı sırayı izler.

## Kurtarılan dosyalar neden başka bir diske gitmek zorunda?

Silinen dosyalar hâlâ dosya sisteminin artık boş saydığı alanda durur. O diske yazılan her şey, kurtarılan dosyaların kendisi bile, o alana düşüp kurtarmaya çalıştığınız şeyi yok edebilir. Unlost fiziksel disk numaralarını karşılaştırır; bu yüzden aynı diskin iki bölümü de (örneğin tek bir USB bellekteki `E:` ve `F:`) reddedilir.

## Sürücüm bir SSD. Umut var mı?

Çoğu zaman sabit diske göre daha az. Windows SSD'lerde boşalan alan için TRIM gönderir ve çoğu SSD bu blokları kısa süre sonra temizler. Unlost genellikle adları yine bulur, ancak içerik sıfır olarak okunur; bu dosyalar "Sıfırlanmış" olarak işaretlenir. Dosya kaydının içinde saklanan küçük dosyalar ("resident") kalabilir; sağlam kopyalar (Geri Dönüşüm Kutusu, gölge kopyalar, bulut çöp kutuları) bundan etkilenmez. USB bellekler ve SD kartlar genellikle TRIM almaz.

## Durumlar ne anlama geliyor?

- **Sağlam kopya:** Windows'un sakladığı eksiksiz bir kopya (Geri Dönüşüm Kutusu, gölge kopya).
- **Büyük olasılıkla sağlam:** dosyanın alanını başka bir şey kullanmıyor ve içeriği yerinde. Bir çözücüyle kontrol edilmedi; emin olmak için açıp bakın.
- **Kısmi:** alanın bir kısmı yeniden kullanılmış, dosyanın başı kendi türüne benzemiyor ya da bazı kısımları okunamadı. `PARTIAL_<ad>` olarak kaydedilir.
- **Sıfırlanmış:** alan boş ama sıfır olarak okunuyor; TRIM sonrasında tipiktir. Yine de kurtarırsanız `ZEROED_<ad>` olarak kaydedilir.
- **Üzerine yazılmış:** alanının neredeyse tamamında artık başka dosyalar var.
- **Yalnızca ad:** ad ve tarihler duruyor, ama içeriğin nerede olduğu bilgisi yok.
- **Burada kurtarılamaz:** EFS ile şifreli, yalnızca çevrimiçi bir bulut dosyası ya da bu sürümün okuyamadığı bir biçimde sıkıştırılmış.

Her dosya ayrıca sayılarla birlikte gerekçesini gösterir.

## Unlost neden yönetici erişimi istiyor?

Bir sürücüyü doğrudan okumak ve gölge kopyaları açmak bunu gerektirir. Unlost uygulamanın tamamını yönetici olarak çalıştırmaz: ayrı bir yardımcı izni alır, okur ve veriyi yalnızca sizin hesabınızın açabildiği bir kanal üzerinden pencereye geri gönderir. Yardımcı hiç dosya yazmaz. Geri Dönüşüm Kutusu'ndan kurtarma ve disk imajı taramak yönetici erişimi gerektirmez.

## Telefonumdaki dosyaları kurtarabilir mi?

Telefon USB ile bağlıyken hayır. Telefonlar medya aygıtı (MTP) olarak bağlanır: Windows dosyalarını gösterir ama depolama alanını göstermez; bu yüzden hiçbir bilgisayar aracı onları silinmiş dosyalar için tarayamaz. Telefonun Son silinenler albümüne ve bulut fotoğraf çöp kutunuza bakın (Google Fotoğraflar yedeklenmiş öğeleri 60 gün tutar). Dahili depolama olarak ayarlanmamış, çıkarılabilir bir SD kart bir kart okuyucuda taranabilir.

## Geri Dönüşüm Kutusu öğeleri için neden "Geri yükle" yok?

Özgün konuma geri yüklemek aynı sürücüye yazar ve hâlâ isteyebileceğiniz başka silinmiş dosyaların üzerine yazabilir. Unlost öğeleri başka bir diske kopyalar ve Geri Dönüşüm Kutusu'nu olduğu gibi bırakır. Sonrasında Gezgin'den yine geri yükleyebilirsiniz.

## Unlost taramadan önce geri yükleme noktası ya da gölge kopya oluşturur mu?

Hayır, asla. Oluşturmak, kurtarılan birime yazmak demektir.

## Unlost internete bir şey gönderiyor mu?

Hayır. Hiçbir ağ bağlantısı kurmaz. "Diğer yerler" sayfasındaki yardım bağlantıları yalnızca tıkladığınızda tarayıcınızda açılır.

## Tarama 1.000.000 dosyada durdu

Yoğun kullanılan bir Windows sürücüsünde milyonlarca silinmiş dosya kaydı olabilir; çoğu geçici dosyadır. Bu sürüm ilk bir milyonu bellekte tutar. Daha küçük bir birimin imaj dosyasını kullanın ya da planlanan oturum deposunu bekleyin.

## Rapor nerede?

Kurtarma klasöründe, `unlost-report.csv` ve `unlost-report.json` olarak: özgün yol, kurtarılan yol, boyut, yöntem, durum, gerekçe, SHA-256 ve yazılan dosyanın geri okunup eşleşip eşleşmediği.

## Unlost'u kurtarma yaptığım diskten çalıştırmak güvenli mi?

Her program çalışırken kendi diskine biraz yazar (Windows Prefetch, uygulamanın kendi profili). En iyi şans için taşınabilir sürümü bir USB bellekten ya da başka bir diskten çalıştırın. Seçtiğiniz disk uygulamanın kendi diskiyse ilk ekran uyarır.
