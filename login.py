username = input("Masukan Username :")
password = int(input("Masukan Passwrd :"))

if username == "Naufal@gmail.com" and password == 12345:
    status = "Berhasil Login !!!"

elif username != "Naufal@gmail.com" and password == 12345:
    status = "Username Salah"

elif username == "Naufal@gmail.com" and password != 12345:
    status = "Password Salah"

elif username != "Naufal@gmail.com" and password != 12345:
    status = None

print("===== LOGIN ADMIN =====")

if status:
    print("Hallo", username)
    print(status)

else:
    print("Username dan Password Salah!")
