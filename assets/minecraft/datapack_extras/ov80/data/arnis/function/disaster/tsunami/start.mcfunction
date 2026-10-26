scoreboard players set #wv_max arnis.ds 5
execute if score #pass arnis.ds matches 2.. run scoreboard players set #wv_max arnis.ds 7
scoreboard players operation #wv_max arnis.ds < #k_tsunami arnis.ds
scoreboard players set #wv_step arnis.ds 3
scoreboard players set #wv_hold arnis.ds 20
scoreboard players set #wv_fall arnis.ds 3
function arnis:disaster/water/begin
function arnis:disaster/tsunami/level
