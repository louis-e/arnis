function arnis:outbreak/clear
scoreboard players set #state arnis.ob -1
scoreboard objectives setdisplay sidebar
bossbar set arnis:outbreak visible false
tellraw @a {"text":"Outbreak stopped. Start it again from the pause menu.","color":"gray"}
