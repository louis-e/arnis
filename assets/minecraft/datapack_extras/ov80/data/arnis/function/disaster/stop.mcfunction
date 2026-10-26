function arnis:disaster/clear
scoreboard players set #state arnis.ds -1
scoreboard objectives setdisplay sidebar
bossbar set arnis:disaster visible false
tellraw @a {"text":"Disasters stopped. Start them again from the pause menu.","color":"gray"}
